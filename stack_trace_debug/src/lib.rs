use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields, FieldsNamed, FieldsUnnamed, Ident};

/// `#[stack_trace_debug]` — use alongside `#[derive(Debug, Snafu)]`.
///
/// Snafu conventions assumed:
///   - `#[snafu(display("..."))]`  → Display (snafu generates it)
///   - `location: Location`        → snafu::Location, appended to each line
///   - `source: T`                 → internal StackError, chain continues
///   - `#[snafu(source)]`
///     `error: T`                  → external error, printed as leaf
///   - tuple variants              → external leaf, no location
///   - unit variants               → no source
///
/// Does NOT generate Debug or Display — snafu + derive(Debug) own those.
/// Only generates `impl StackError`.
///
/// # Example
/// ```ignore
/// #[derive(Debug, Snafu)]
/// #[stack_trace_debug]
/// pub enum Error {
///     #[snafu(display("missing key '{key}'"))]
///     MissingKey { key: String, location: Location },
///
///     #[snafu(display("filesystem error"))]
///     FileSystem {
///         #[snafu(source)]
///         error: std::io::Error,
///         location: Location,
///     },
///
///     #[snafu(display("catalog '{catalog_name}' failed"))]
///     Catalog {
///         catalog_name: String,
///         location: Location,
///         source: CatalogError,  // internal — chain continues
///     },
/// }
/// ```
#[proc_macro_attribute]
pub fn stack_trace_debug(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let crate_path: syn::Path = syn::parse_str("::my_error").unwrap();

    let input = parse_macro_input!(item as DeriveInput);
    let enum_name = &input.ident;

    let variants = match &input.data {
        Data::Enum(e) => &e.variants,
        _ => {
            return syn::Error::new_spanned(&input, "#[stack_trace_debug] only works on enums")
                .to_compile_error()
                .into();
        }
    };

    let mut debug_fmt_arms = Vec::<TokenStream2>::new();
    let mut next_arms = Vec::<TokenStream2>::new();

    for variant in variants {
        let vname = &variant.ident;

        match &variant.fields {
            // ── Named fields ─────────────────────────────────────────────────
            Fields::Named(FieldsNamed { named, .. }) => {
                let fields: Vec<_> = named.iter().collect();

                let has_location = fields.iter().any(|f| name_is(f, "location"));
                let has_source = fields.iter().any(|f| name_is(f, "source"));
                // snafu marks external errors with #[snafu(source)] + field named `error`
                let has_error = fields.iter().any(|f| name_is(f, "error"));

                let location_bind = has_location.then(|| quote! { location, });
                let source_bind = has_source.then(|| quote! { source, });
                let error_bind = has_error.then(|| quote! { error, });

                // snafu::Location implements Display as "file:line:col"
                let location_fmt = if has_location {
                    quote! {
                        buf.push(format!("{}: {}\n   -> {}", layer, self, location));
                    }
                } else {
                    quote! {
                        buf.push(format!("{}: {}", layer, self));
                    }
                };

                let recurse = if has_source {
                    // internal — delegate to next layer's debug_fmt
                    quote! { source.debug_fmt(layer + 1, buf); }
                } else if has_error {
                    // external — print as leaf, stop chain
                    quote! { buf.push(format!("{}: {}", layer + 1, error)); }
                } else {
                    quote! {}
                };

                debug_fmt_arms.push(quote! {
                    #enum_name::#vname {
                        #location_bind
                        #source_bind
                        #error_bind
                        ..
                    } => {
                        #location_fmt
                        #recurse
                    }
                });

                let next_body = if has_source {
                    quote! { Some(source as &dyn #crate_path::StackError) }
                } else {
                    quote! { None }
                };

                next_arms.push(quote! {
                    #enum_name::#vname { #source_bind .. } => { #next_body }
                });
            }

            // ── Tuple variant — external leaf, no location ────────────────────
            Fields::Unnamed(FieldsUnnamed { unnamed, .. }) => {
                let count = unnamed.len();
                let bindings: Vec<Ident> = (0..count)
                    .map(|i| syn::parse_str(&format!("_f{}", i)).unwrap())
                    .collect();

                let leaf = if count == 1 {
                    let b = &bindings[0];
                    quote! { buf.push(format!("{}: {}", layer + 1, #b)); }
                } else {
                    let lines = bindings.iter().enumerate().map(|(i, b)| {
                        quote! { buf.push(format!("{}  [{}]: {}", layer, #i, #b)); }
                    });
                    quote! { #( #lines )* }
                };

                debug_fmt_arms.push(quote! {
                    #enum_name::#vname( #( ref #bindings ),* ) => {
                        buf.push(format!("{}: {}", layer, self));
                        #leaf
                    }
                });
                next_arms.push(quote! {
                    #enum_name::#vname(..) => { None }
                });
            }

            // ── Unit variant ──────────────────────────────────────────────────
            Fields::Unit => {
                debug_fmt_arms.push(quote! {
                    #enum_name::#vname => {
                        buf.push(format!("{}: {}", layer, self));
                    }
                });
                next_arms.push(quote! {
                    #enum_name::#vname => { None }
                });
            }
        }
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        #input

        impl #impl_generics #crate_path::StackError
            for #enum_name #ty_generics #where_clause
        {
            fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
                match self {
                    #( #debug_fmt_arms )*
                }
            }

            fn next(&self) -> Option<&dyn #crate_path::StackError> {
                match self {
                    #( #next_arms )*
                }
            }
        }

        impl #impl_generics ::std::fmt::Debug
            for #enum_name #ty_generics #where_clause
        {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                use #crate_path::StackError;
                let mut buf = Vec::new();
                self.debug_fmt(1, &mut buf);
                write!(f, "\n{}\n", buf.join("\n"))  // ← leading newline
            }
        }
    };

    expanded.into()
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn name_is(field: &syn::Field, name: &str) -> bool {
    field.ident.as_ref().map(|i| i == name).unwrap_or(false)
}
