# Architecture

cbld is a single Rust binary. It shells out to tools the OS already has (`clang`, `clang-format`, `clang-tidy`, `git`, `ar` / `llvm-ar` / `lib.exe`, `curl` or PowerShell) instead of embedding HTTP, VCS, or compiler libraries.

`cbld fmt` runs `clang-format` (`--check` is `--dry-run --Werror`). `cbld lint` runs `clang-tidy` (`--deny-warnings` is `-warnings-as-errors=*`). `cbld check` is Clang `--analyze`, not tidy.

## Layout

A package is a directory with `cbld.toml` and a source tree (default `src/`). Public headers may live in `include/`. Workspaces list members under `[workspace] members`; each member is built as its own package.

Standard C++20 module interfaces use `.cppm`, `.ccm`, `.cxxm`, `.c++m`,
`.ixx`, or `.mxx`. The package-local module planner builds BMI files in
`target/.../obj/<package>/.../modules`, topologically orders importers, and
passes `-fmodule-file` to consumers. Module packages bypass the global static
library cache until BMI artifacts can be cached safely.

## Dependencies

`gh:owner/repo` maps to `https://github.com/owner/repo.git`. Overlay recipes (shipped in the binary, overridable via `~/.cbld/cbld-libs` from `cbld sync`) tell cbld how to treat an upstream tree that has no `cbld.toml`: `kind`, `source_dir`, `include`/`exclude`, `include_dirs`. The clone is never rewritten. If the clone has a `cbld.toml`, that file wins. The official list is on [Packages](/packages).

`kind = "header"` exposes include paths only — no archive. Compiled `lib` dependencies are archived and linked into the consumer executable.

Resolution walks each dependency's effective manifest (clone or overlay) and compiles transitives first. Versions are git tags; `cbld.lock` pins commit SHAs.

`[target.<triple>]` presets provide an existing package-relative `sysroot`.
The selected `--target` or `[package] target` is propagated to dependency
compilation and linking. Cross-target outputs are isolated under
`target/<triple>/` and are never executed by `run`, `test`, or `bench`.

`cbld vendor` copies the locked graph into `third_party/` for offline builds. Overlay recipes still apply to vendored copies (looked up by git URL).

`[features]` on a package expands to `-DCBLD_FEATURE_*` defines. A dependency table `{ version = "1.0", features = ["ssl"] }` passes those names into the dependency's own feature set when compiling it. Features do not enable or disable dependency entries.

## Build cache

Only **static libraries** go in `~/.cbld/cache/prebuilt/{hash}/`. The hash covers sources, compiler flags, and host OS/arch. Executables are project-specific and are not cached globally.

cbld home is `$CBLD_HOME`, else `$HOME/.cbld`, else `%USERPROFILE%\.cbld` on Windows.

## JSON

`cbld build --json`, `cbld test --json`, `cbld bench --json`, and `cbld doctor --json` use a small in-tree JSON writer (`src/json.rs`). There is no `serde_json` dependency. Other commands accept `--json` but ignore it.

`cbld test` discovers C/C++ test sources, compiles them in parallel, links a
test executable, and runs it. A library package is built first and its artifact
is linked into the test executable. Test output is passed through unchanged;
the runner recognizes common GoogleTest, Catch2, and doctest summary lines and
falls back to the process exit status for plain test programs.

`cbld bench` uses the same compiler and package-library linking path, then
passes `--benchmark_format=console` to the benchmark executable. Parsed
Google Benchmark-compatible rows expose real and CPU time in nanoseconds for
machine consumers.

## Archivers

- Linux: `ar rcsD`
- macOS: `llvm-ar rcsD`, then BSD `ar rcs`
- Windows: `llvm-ar rcsD`, then `lib.exe`

`cbld doctor` reports whichever of those is on PATH, not only GNU `ar`.
