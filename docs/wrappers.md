# Linux executable wrappers

## Installed layout

The wrapper resolves its installed path through `/proc/self/exe` and runs
`.<name>-wrapped` beside it. Install the executable under that hidden name and
the shim at the tool's public path. The shim passes through the original
`argv[0]` by default. A custom adapter can call `Tool::use_installed_name` to
supply the installed public path to the executable.

For Nix packages, add `install-adjacent-wrapper-hook` from this flake to
`nativeBuildInputs` and call `installAdjacentWrapper` with the installed
vendor executable and the prebuilt shim.

```nix
postInstall = ''
  installAdjacentWrapper "$out/bin/my-tool" ${lib.getExe myToolAdapter}
'';
```

Multiple launchers can share a runner built with
`wrap_tool!(launcher => adapter)`. Put any private launcher options first,
then the public launcher's absolute path, then the caller's arguments. Each
private option is one argument beginning with `--`, so pass values in the
same argument, for example `--backend=/some/bin`. The adapter reads these
options through `Tool::launcher_options`. The runner consumes them and the
absolute path, then locates the neighboring `.<name>-wrapped` executable.
The remaining argument bytes and the original `argv[0]` follow the same
rules as for a standalone shim.

## Writing an adapter

Use `wrap_tool!` once in each wrapper executable. Use
`wrap_tool!(Some("my-tool"), adapter)` to specify the name or
`wrap_tool!(None, adapter)` to use the installed filename. The macro installs a
startup hook that captures closed standard descriptors and the inherited
`SIGPIPE` disposition before Rust initializes its runtime.

Choose the launch and editing APIs according to the tool's policy.

| Use case | Adapter call site |
| --- | --- |
| Pass the caller's arguments through | Call `tool.forward()`. See the [pass-through shim](../src/bin/tool-wrapper.rs). |
| Wait and relay exit status | Call `tool.supervise()` with the current arguments. Use `Tool::run` when the adapter needs to configure the command or manage a scratch lease. |
| Run custom post-processing after a successful exit | Call `Tool::run_then` with the configured command, optional scratch directory, and callback. |
| Inspect or rewrite nested response files | Use `response::visit` or `response::rewrite` with the tool's grammar. See the [GNU ar response-file example](../examples/rewrite-ar.rs). |
| Supply rewritten inputs | Stage bytes with `InputFiles::stage`, attach the descriptors to the argument list with `PreparedArgs`, then launch. See the [staged input example](../examples/stage-input.rs). |
| Fill an input after inspecting the arguments | Call `InputFiles::reserve` for a path and a file handle. Fill the file through the handle before launch. |
| Capture seekable output | Pass a clone of `files::anonymous_file` to `Command::stdout` and read the original handle after the child exits. |
| Normalize a generated file | Call `Tool::edit_output` with the command, expected output, and a callback. See the [archive example](../examples/normalize-ar.rs). |
| Publish a fresh artifact atomically | Enable the `publish` feature and call `Tool::publish_output`. See the [atomic copy example](../examples/publish-output.rs). |
| Normalize bytes returned by a compiler or linker API | Edit `Artifact::Bytes` or call the format-specific helper on the returned bytes. See the [JIT example](../examples/jit). |

A supervised adapter can use the default command directly.

```rust
use std::process::ExitCode;
use tool_wrapper::wrap_tool;

fn main() -> ExitCode {
    wrap_tool!(Some("my-tool"), |tool| tool.supervise())
}
```

The staged input and archive examples can run with `cat` and GNU `ar`. From
the repository root, build them and install the shims in a temporary directory.

```sh
cargo build --locked --release --features wrapper --examples
demo=$(mktemp -d)
install -m 755 target/release/examples/stage-input "$demo/cat"
ln -s "$(command -v cat)" "$demo/.cat-wrapped"
"$demo/cat"

install -m 755 target/release/examples/normalize-ar "$demo/ar"
ln -s "$(command -v ar)" "$demo/.ar-wrapped"
printf 'archive member\n' > "$demo/member"
"$demo/ar" crU "$demo/example.a" "$demo/member"
ar tv "$demo/example.a"
```

The archive example handles `crU` to demonstrate output editing. GNU `ar`
also provides its own deterministic mode through `D`.
[GNU ar command-line reference](https://sourceware.org/binutils/docs/binutils/ar-cmdline.html)

For fixed flags and environment settings in a Nix package, `makeBinaryWrapper`
can generate the executable wrapper during packaging.
[Nixpkgs wrapper hooks](https://github.com/NixOS/nixpkgs/blob/master/doc/stdenv/stdenv.chapter.md)

## Response files

Call `response::visit` to inspect the effective arguments in order. Call
`response::rewrite` to stage changed files and their ancestors and receive
`PreparedArgs`. Apply those arguments with `Tool::with_prepared_args`. When
inspection fails or there are no edits, the original arguments are used.
Errors from the edit callback and staging propagate to the caller.
Use `response::rewrite_required_until` when an inspection or encoding failure
must abort the invocation.
A rejected edit releases the descriptors it staged. Prepared commands retain
their staged inputs and pass those descriptors to the tool and its descendants.
Other commands close the staged descriptors on exec.

Use `response::visit_until` or `response::rewrite_until` when a tool option
bypasses the adapter's work. The callback can return `ControlFlow::Break(())`
to stop before inspecting further arguments or files. A break during rewrite
also discards edits and their staged descriptors, so the caller can forward
the original arguments. Use this for invocations such as `--version` and
`--dryrun` when the adapter leaves them alone.

For other byte-oriented formats, `quoting::split_words` exposes the scanner
used by the GNU response parser. The format supplies its quote
characters and the rule for whether to retain, drop, or reject escaped bytes.
`quoting::push_double_quoted` appends one escaped word to an existing buffer.

The core provides `Gnu` for libiberty-style `@file` arguments. It delegates
ambiguous quoting to the wrapped tool and resolves relative paths from the
working directory. The wrapped tool must accept staged files at `/proc/self/fd`.
Other tools can set `Syntax::FILE_RELATIVE` to resolve includes from the
including file's directory. Replay preserves unchanged nested includes in
a relocated file by writing their absolute paths. `Syntax::prepare_file` handles any additional
path syntax before relocation. For example, the [Clang config shim](../examples/rewrite-clang-config.rs)
expands simple `<CFGDIR>` paths using the original config location. The
example handles explicit `--config=./path.cfg` options in Unix driver mode
and delegates unsupported config syntax to Clang.
[Clang configuration file rules](https://clang.llvm.org/docs/UsersManual.html#configuration-files)

Call `response::option_group` from `Syntax::next` to classify plain words,
required operands, and named response-file options. For options requiring
a value, it handles `--flag=value` and `--flag value` using the original
argument bytes. The adapter supplies the option names and any file-list
separator.

To run the GNU response-file example, reuse the temporary directory and
build from above. This shim changes the first effective `crU` operation to
`crD`, selecting the tool's native deterministic mode.

```sh
install -m 755 target/release/examples/rewrite-ar "$demo/ar"
printf 'crU\n' > "$demo/operation.rsp"
printf '@operation.rsp "replayed archive.a" member\n' > "$demo/outer.rsp"
(cd "$demo" && ./ar @outer.rsp)
ar tv "$demo/replayed archive.a"
```

For a file-relative example, run the Clang shim against a nested config.

```sh
install -m 755 target/release/examples/rewrite-clang-config "$demo/clang"
ln -s "$(command -v clang)" "$demo/.clang-wrapped"
mkdir -p "$demo/config"
printf '@defines.cfg\n' > "$demo/config/root.cfg"
printf '%s\n' '-DWRAPPER_DEMO=0' > "$demo/config/defines.cfg"
printf 'WRAPPER_DEMO\n' |
  (cd "$demo" && ./clang --no-default-config --config=./config/root.cfg -E -P -x c -)
```

The demo prints `1` because only the nested config's macro definition is edited.

## Atomic output publishing

Enable `publish` for a tool that can write a fresh, single-file output.
Pass `Tool::publish_output` the final path, a closure that configures the
command with the staged path, and an artifact editor. It gives the tool the
same basename inside a private directory alongside the destination. After
the tool exits successfully, the publisher edits and syncs the staged file,
then renames it over the destination. Use `PendingOutput` directly when no
subprocess is needed.

Use this mode when replacing the destination inode and passing a different
parent path to the tool fit the adapter's semantics. A tool that embeds its
output directory can embed the staging path. The destination parent must
remain stable. Concurrent successful publishers replace the destination in
rename order. The file is synced before rename. The destination directory
is outside the durability guarantee.

The [copy example](../examples/publish-output.rs) can publish a normalized GNU
archive. Continue in the same demo directory after building the example.

```sh
cargo build --locked --release --features publish --example publish-output
install -m 755 target/release/examples/publish-output "$demo/cp"
ln -s "$(command -v cp)" "$demo/.cp-wrapped"
ar crU "$demo/source.a" "$demo/member"
"$demo/cp" "$demo/source.a" "$demo/published.a"
ar tv "$demo/published.a"
```

## Invocation semantics

- `Tool::command` prepares a command with the exact argument bytes and the
  caller's standard descriptor state. `Tool::command_from` also accepts
  iterators of borrowed byte slices, such as generated flags chained with the
  original arguments. Descriptors closed at wrapper startup stay closed in the
  child even if the adapter configures redirection for them. `Tool::exec`
  replaces the shim. With ordinary invocation, the executable inherits the
  caller's `SIGCHLD` and `SIGPIPE` dispositions.
- `Tool::run` waits while forwarding cancellation signals. In a background
  build it gives the child a new process group and forwards signals to that
  group. In the foreground, terminal signals reach the shared group directly.
  The shim forwards signals sent to its own PID to the immediate child.
  A signal sent explicitly to the shared foreground group can reach the child
  directly and again through the shim.
  The shim itself must be single-threaded and own child reaping. Callers can
  launch separate shim processes in parallel, and the wrapped tool can create
  its own threads and subprocesses. After cleanup,
  `exit_status` returns the child's exit code or re-raises its terminating signal.
  The supervisor waits for the direct child. Descendants receive group signals
  while they remain in the group. If the shim dies, Linux sends `SIGKILL` to
  the direct child through its parent-death signal.
- A process may hold a scratch-directory lease across exec. Another invocation
  can recover a marked scratch directory after a crash once holders of the
  earlier lease exit. `Tool::run_then` passes the lease to the child and drops
  the wrapper's scratch handle before invoking the success callback or
  propagating the exit status. It drops the unused callback before re-raising
  a terminating signal so captured staging resources can be cleaned up.
- `Tool::edit_output` applies an editor after a successful run and checks that
  the executable generated the intended output. It resolves relative paths
  against the command's working directory. Give concurrent invocations separate
  output paths.

## Library interfaces

- [`process`](../src/process.rs) provides the command and supervision APIs.
- [`files`](../src/files.rs) provides anonymous inputs, file descriptor paths,
  bounded reads, and generated-file checks. `mapped_working_directory` returns an
  absolute prefix-mapped alias after verifying it names the current directory.
- [`response`](../src/response.rs) traverses response files with configurable
  depth and byte limits and stages rewritten arguments for launch.
- [`quoting`](../src/quoting.rs) scans and encodes byte-oriented quoted words.
- [`output`](../src/output.rs) publishes a fresh file through a same-filesystem
  rename when the `publish` feature is enabled.
- [`scratch`](../src/scratch.rs) provides stable identifiers and locked scratch
  directories. The caller selects a directory and passes its lease to
  `Tool::run` when descendants need to retain the lock.
- [`artifact`](../src/artifact.rs) edits named ELF64 sections and normalizes
  regular `ar` archive timestamps and ownership after validating the member
  layout. These edits preserve offsets. `Artifact::rewrite` loads the whole
  artifact and can resize a file in place or before atomic publication.
  A rewrite of a borrowed buffer must preserve its length.
- [`ByteEdits`](../src/byte_edits.rs) applies ordered replacements or
  insertions to a byte slice and copies the untouched ranges verbatim.
- [`path_prefix_map`](../src/path_prefix_map.rs) parses and applies
  `BUILD_PATH_PREFIX_MAP`.
- [`nix_store`](../src/nix_store.rs) provides a fixed-width candidate hash
  scrubber for metadata whose references the adapter has determined are
  incidental.
