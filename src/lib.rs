//! Implementation of GreptimeDB's stacked error pattern — macro edition.
//!
//! Key ideas:
//! - Each error variant carries a `Location` (file, line, col)
//! - `StackError` trait walks the chain and renders it
//! - Internal errors use field name `source` (implement StackError)
//! - External errors use field name `error` (plain std::error::Error)
//! - Two display modes: debug (full stack) and user-facing (brief)

pub use stack_trace_debug::stack_trace_debug;

// ── StackError trait ──────────────────────────────────────────────────────────

/// Trait for errors that form a linked chain with location info.
pub trait StackError: std::error::Error {
    /// Write this layer's debug line into `buf`.
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>);

    /// Walk to the next *internal* error in the chain (field named `source`).
    fn next(&self) -> Option<&dyn StackError>;

    /// Walk to the deepest internal error.
    fn last(&self) -> &dyn StackError
    where
        Self: Sized,
    {
        let mut cur: &dyn StackError = self;
        while let Some(next) = cur.next() {
            cur = next;
        }
        cur
    }
}

// ── Render helpers ────────────────────────────────────────────────────────────

/// Full stacked debug format, one line per layer.
///
/// ```
/// 0: Outer error message, at src/main.rs:42:10
/// 1: Inner error message, at src/main.rs:88:5
/// 2: serde_json(invalid character at position 1)
/// ```
pub fn format_stack(err: &dyn StackError) -> String {
    let mut buf = Vec::new();
    err.debug_fmt(0, &mut buf);
    buf.join("\n")
}

/// Terse user-facing format:  `KIND - REASON ([EXTERNAL CAUSE])`
pub fn format_user(err: &dyn StackError) -> String {
    // Outermost Display = what the user triggered
    let outer = err.to_string();

    // Walk to innermost internal error
    let mut cur: &dyn StackError = err;
    while let Some(next) = cur.next() {
        cur = next;
    }
    let inner = cur.to_string();

    // The innermost internal error may itself wrap an *external* cause
    // (std::error::Error source that doesn't impl StackError).
    // std::error::Error::source() gives us that.
    let external = std::error::Error::source(cur)
        .map(|e| format!(" ({})", e))
        .unwrap_or_default();

    if outer == inner {
        format!("{}{}", outer, external)
    } else {
        format!("{} - {}{}", outer, inner, external)
    }
}
