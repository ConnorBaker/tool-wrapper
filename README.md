# tool-wrapper

A Rust library and binary for wrapping Linux executables. It provides direct
execution, optional child supervision, anonymous inputs, response-file replay,
locked scratch directories, and post-run artifact editing. The library can
wrap any tool that fits the adjacent-executable layout.

## Build with Nix

Run `nix build` to build the generic `tool-wrapper` shim as a static musl
executable on aarch64 or x86-64 Linux.

Use `nix develop` for the Rust toolchain and Git hooks, `nix fmt` for Nix
and Rust formatting, and `nix flake check` for Nix lints, Clippy, and the
wrapper tests.

## Wrap an executable

Build the pass-through shim with Rust 1.98 or newer.

```sh
cargo build --locked --release --features wrapper --bin tool-wrapper
```

In your staged install tree, use the public name for the shim and
`.<name>-wrapped` for the original executable. For example, install the shim
as `bin/my-tool` and the executable as `bin/.my-tool-wrapped`. The shim
replaces itself with the executable and forwards arguments byte for byte.

To inspect arguments, supervise the child, or edit its output, define an
adapter in Rust and enable the `wrapper` feature on the dependency.

```toml
[dependencies]
tool-wrapper = { path = "../tool-wrapper", features = ["wrapper"] }
```

Call [`wrap_tool!`](./src/lib.rs) in the adapter's entry point and choose
`Tool::exec`, `Tool::run`, or `Tool::edit_output` according to what the tool
needs. The [wrapper guide](./docs/wrappers.md) covers process semantics and
the Nix setup hook for installing shims.

The `wrapper` feature enables the shim binary and the Linux process and
descriptor APIs, which use `libc` and `rustix`. The `publish` feature adds
atomic publication of fresh output files, using `tempfile` to stage each
file in a private directory on the destination filesystem.

## Edit artifacts directly

Use `tool-wrapper` with default features for byte-preserving path prefix
maps, in-place edits to named little-endian ELF64 sections, and normalization
of regular `ar` archive dates and ownership. The artifact APIs run on Unix and
accept mutable byte slices or open files.

## Tests

Run these on Linux.

```sh
cargo test --locked --lib
cargo test --locked --all-features --all-targets
```
