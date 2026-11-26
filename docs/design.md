# Design notes

## Dependency evaluation

| Need | Crates and findings |
| --- | --- |
| Find mutable ELF sections | The `object` reader handles extended section numbering, names, and file ranges. `ReadCache` reads the metadata it needs from a file via a positioned-I/O adapter. This preserves the caller's seek offset. The wrapper validates all selected ranges before writing. [object ELF reader](https://docs.rs/object/latest/object/read/elf/index.html), [ReadCache](https://docs.rs/object/latest/object/read/struct.ReadCache.html) |
| Adapt positioned I/O | A prototype with `positioned-io` 0.3.5 used `SizeCursor<Slice<&File>>` in `ReadCache` and passed the seek-offset regression test. It removed 35 local lines and added four lockfile entries, including three Windows-only packages. The current implementation uses `std::os::unix::fs::FileExt`. [SizeCursor](https://docs.rs/positioned-io/0.3.5/positioned_io/struct.SizeCursor.html), [Slice](https://docs.rs/positioned-io/0.3.5/positioned_io/struct.Slice.html) |
| Other ELF and debug readers | `elf::ElfStream` is a viable smaller API for an ELF-only consumer. It opens a seekable input and reads both section and program headers. `goblin`'s convenience ELF parser expects a byte slice, and a mapping through `memmap2` requires the file to remain valid for the lifetime of the map. `gimli` can read or rebuild standard DWARF if a future feature needs to interpret it. Arbitrary DWARF path changes can change section sizes and references, so that requires repacking the artifact and validating the resulting metadata. [elf](https://docs.rs/elf/latest/elf/struct.ElfStream.html), [goblin](https://docs.rs/goblin/latest/goblin/elf/struct.Elf.html), [memmap2](https://docs.rs/memmap2/latest/memmap2/struct.Mmap.html), [gimli](https://docs.rs/gimli/latest/gimli/) |
| Raw byte text | `bstr::ByteSlice` provides byte-preserving delimiter searches and splits. `memchr` is already part of the `object` dependency graph. `regex::bytes` and parser combinators become more useful with larger grammars. [bstr](https://docs.rs/bstr/latest/bstr/trait.ByteSlice.html), [memchr](https://docs.rs/memchr/latest/memchr/), [regex bytes](https://docs.rs/regex/latest/regex/bytes/), [winnow](https://docs.rs/winnow/latest/winnow/) |
| Command-line parsing | `lexopt` and `clap` suit shims that expose their own options. A transparent adapter needs to forward unknown vendor arguments in order. See the response-file notes below for vendor-format parsing. [lexopt](https://docs.rs/lexopt/latest/lexopt/), [clap](https://docs.rs/clap/latest/clap/) |
| Archive headers | The editor uses `object::archive::Header` for the record layout and derives the date offset with `offset_of!`. `ArchiveFile::members` skips special members such as the GNU symbol table. The editor walks the headers itself so it can normalize that member and leave the archive layout and offsets intact. [object archive layout](https://github.com/gimli-rs/object/blob/master/src/archive.rs), [object archive reader](https://docs.rs/object/latest/object/read/archive/struct.ArchiveFile.html) |
| Linux primitives | `rustix` supplies owned anonymous descriptors, typed open flags and PIDs, retries on interrupted opens, and parent-death and foreground-group operations. The standard library supplies file locking, positioned I/O, and process launch. Reproducible scratch directories also need a repeatable path, ownership checks, and a lock inherited by vendor subprocesses. [rustix FS](https://docs.rs/rustix/latest/rustix/fs/), [rustix process](https://docs.rs/rustix/latest/rustix/process/), [Rust File API](https://doc.rust-lang.org/stable/std/fs/struct.File.html) |
| Process supervision | `libc::sigwaitinfo` exposes signal origin, which the wrapper uses to avoid delivering a terminal signal twice to a foreground child. Rustix puts the libc-level signal APIs outside its supported surface. `process-wrap` can manage process groups and `nix::sys::signalfd` can expose sender information, but the wrapper still needs the original descriptor and signal state, the inherited scratch lease, and propagation of the child's status or terminating signal. [rustix signal API boundaries](https://docs.rs/crate/rustix/latest/source/src/not_implemented.rs), [process-wrap](https://docs.rs/process-wrap/latest/process_wrap/), [nix SignalFd](https://docs.rs/nix/latest/nix/sys/signalfd/struct.SignalFd.html) |
| Prefix maps | The core implements reproducible-builds escaping, last-match precedence, and path-component matching on byte paths. Adapters choose which inputs use the map. Rust's own remapping flags affect rustc-generated paths. [BUILD_PATH_PREFIX_MAP specification](https://reproducible-builds.org/specs/build-path-prefix-map/), [rustc remapping](https://doc.rust-lang.org/rustc/remap-source-paths.html) |

## Response-file reuse

A response-file inspector can flatten the parsed words to find the effective
output or other options. An adapter that changes an argument and then invokes
the original tool must also retain the outer argument positions and nested
file boundaries. Replaying a large expanded list as argv can exceed the
kernel's size limit. The API already shares temporary input storage and
descriptor ownership across grammars.

- `winnow_args::response::expand` handles recursive GNU-style `@file` input and
  yields borrowed byte words for inspection. Its public expansion returns a
  flat list. Its independent `split` function takes UTF-8 text.
  [winnow-args response API](https://docs.rs/winnow-args/latest/winnow_args/response/index.html)
- `argfile` expands a single prefix and reads the file as UTF-8 before invoking
  its parser. It fits applications that own a compatible response-file format.
  [argfile implementation](https://docs.rs/argfile/latest/src/argfile/lib.rs.html)
- `shlex::bytes` is useful for a deliberately POSIX-shell grammar. GCC
  supports recursive `@file` and both quote types with its own quoting
  and escaping rules.
  [shlex byte API](https://docs.rs/shlex/latest/shlex/bytes/index.html),
  [GCC response-file rules](https://gcc.gnu.org/onlinedocs/gcc/Overall-Options.html)

In a local probe, winnow-args 0.6.5 and GCC 13.3 agreed on the quoted values
from a nested `@file`. Winnow-args also preserved a non-UTF-8 input word
that argfile rejected. The POSIX shell parser treated an unquoted `#` word
as a comment.

The core shares traversal, limits, and replay across grammars. A grammar
supplies option grouping, response-file spelling, and quoting. The GNU parser handles the common subset of libiberty's quoting
rules and leaves ambiguous backslash sequences to the original tool.
Inspection-only adapters can reuse the traversal and launch with the original
arguments. Replay supports working-directory and including-file lookup bases.
When a file-relative response is relocated, its unchanged nested references
become absolute. Grammar hooks materialize additional file-local syntax such
as Clang's `<CFGDIR>`. [Libiberty parser](https://github.com/gcc-mirror/gcc/blob/master/libiberty/argv.c) · [Clang configuration files](https://clang.llvm.org/docs/UsersManual.html#configuration-files)

The optional `publish` feature uses `tempfile::TempDir` for private sibling
directories and normal-return cleanup. Keeping the producer's basename
preserves extension-based behavior. `NamedTempFile::persist` can rename an
already-created temporary file atomically. The producer here gets a fresh
path, then the wrapper validates, edits, syncs, and renames the resulting
file. [tempfile builder](https://docs.rs/tempfile/latest/tempfile/struct.Builder.html), [NamedTempFile::persist](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist)

## Rust implementation choices

The response-file rewriter uses `Cow<[Argument]>` to borrow argv until
the first replacement. Nested files already have owned token lists and can
be edited in place. The process supervisor uses `rustix`'s typed child PID,
signals, and process-group delivery helpers. It keeps the signal mask and
sender check tied to the Linux wrapper's invocation protocol.

The response walker uses Rust 1.98's `slice::subslice_range` to locate
replacement spans in the original bytes. The process and scratch code also
use standard library file locking and edition 2024 let chains.
[Rust slice API](https://doc.rust-lang.org/stable/std/primitive.slice.html#method.subslice_range),
[Rust file locking API](https://doc.rust-lang.org/stable/std/fs/struct.File.html#method.lock),
[Rust let chains](https://blog.rust-lang.org/2025/06/26/Rust-1.88.0/)
