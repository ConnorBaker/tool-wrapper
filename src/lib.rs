//! Byte-preserving artifact edits and optional Linux executable wrapping.

pub mod artifact;
mod byte_edits;
pub use byte_edits::ByteEdits;
#[cfg(feature = "wrapper")]
pub mod files;
pub mod nix_store;
#[cfg(feature = "publish")]
pub mod output;
pub mod path_prefix_map;
#[cfg(feature = "wrapper")]
pub mod process;
pub mod quoting;
#[cfg(feature = "wrapper")]
pub mod response;
#[cfg(feature = "wrapper")]
pub mod scratch;

/// Run an adapter as a standalone CLI shim, preserving the caller's process state.
///
/// Pass `Some("tool")` to select the neighboring `.tool-wrapped` executable or
/// `None` to derive the name from the installed shim. Use
/// `wrap_tool!(launcher => adapter)` to accept private options starting with
/// `--` and then the absolute launcher path before the caller's arguments.
/// Use this macro once in the binary. Applications that only edit artifacts
/// never install the pre-main hook.
#[cfg(feature = "wrapper")]
#[macro_export]
macro_rules! wrap_tool {
    (launcher => $adapt:expr $(,)?) => {
        $crate::wrap_tool!(@invoke $crate::process::Invocation::Launcher, $adapt)
    };
    ($name:expr, $adapt:expr $(,)?) => {
        $crate::wrap_tool!(@invoke $crate::process::Invocation::Standalone($name), $adapt)
    };
    (@invoke $invocation:expr, $adapt:expr) => {{
        // Rust fills closed stdio descriptors and sets SIGPIPE before main.
        #[used]
        #[unsafe(link_section = ".init_array")]
        static SAVE_INVOCATION: extern "C" fn() = $crate::process::save_invocation;
        $crate::process::wrapper_main($invocation, $adapt)
    }};
}

#[cfg(test)]
mod tests;
