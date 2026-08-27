# CLI Reference

Global flags: `-v` / `--verbose`, `-q` / `--quiet`, `--json` (honored by `build` and `doctor` only).

## `build`

Compile the package and its dependencies.

```bash
cbld build
cbld build --release -j 8
cbld build --from vendor/foo --ignore-warnings
cbld build --json
```

Flags: `--release`, `-o` / `--output`, `-j` / `--jobs`, `--manifest-path`, `--features`, `--no-default-features`, `--trace`, `--target`, `--from`, `--ignore-warnings`.

## `run`

Build (if needed) and execute the binary. Accepts the same flags as `build`. `--json` is accepted and ignored.

```bash
cbld run
```

## `init`

Scaffold a new package (`cbld.toml` version `0.1.0` plus `src/main.cpp`).

```bash
cbld init
cbld init my-app --lib --c
```

## `check`

Run Clang's static analyzer without producing object files or linking.

```bash
cbld check -j 4 --manifest-path .
```

At a workspace root, `check` / `fmt` / `lint` run on every `[workspace] members` entry.

## `fmt`

clang-format over the package's sources and public headers (`include/`). Same idea as `cargo fmt`.

```bash
cbld fmt
cbld fmt --check
```

`--check` fails if any file would change. Style comes from `.clang-format` in the package root when present; otherwise LLVM. Needs `clang-format` on PATH.

## `lint`

clang-tidy over the same files. Same idea as `cargo clippy`. `cbld check` is still Clang's static analyzer (`--analyze`); this command is extra lints.

```bash
cbld lint
cbld lint --deny-warnings
```

`--deny-warnings` treats every finding as an error (`clippy -D warnings`). A `.clang-tidy` in the package root overrides the default check set (`clang-diagnostic-*`, `bugprone-*`, `performance-*`). Needs `clang-tidy` on PATH. Flags: `--manifest-path`, `--features`, `--no-default-features`, `--target`.

## `update`

Re-resolve dependencies and rewrite `cbld.lock` (does not touch the package index).

```bash
cbld update
cbld update http_parser
```

The optional argument is the **package name** (last path segment of `gh:owner/repo`).

## `sync`

Download a package index into `~/.cbld/cbld-libs` (TOML overlay recipes or legacy `shorthand <url>` lines). Overlay recipes for `nlohmann/json`, `cJSON`, and `fmt` already ship in the binary.

```bash
export CBLD_LIBS_URL=https://example.com/cbld-libs.toml
cbld sync
```

Requires `CBLD_LIBS_URL`. There is no default public registry. Overlay recipes for the libraries on [Packages](/packages) already ship in the binary. Host `registry/cbld-libs.toml` yourself to add or override recipes. The site still serves the legacy line-format [`/cbld-libs`](/cbld-libs) and the TOML copy [`/cbld-libs.toml`](/cbld-libs.toml).

## `doctor`

Diagnose the local toolchain (`clang`, `clang-format`, `clang-tidy`, archiver, headers, fetch tools) and list builtin overlay recipes.

```bash
cbld doctor
cbld doctor --json
```

## `vendor`

Copy every locked dependency (including transitives) into `third_party/`.

```bash
cbld vendor --manifest-path .
```

## `migrate`

Generate a starter `cbld.toml` from an existing CMake project.

```bash
cbld migrate --from=cmake --path .
```

Only `--from=cmake` is implemented (reads `CMakeLists.txt`). Include directories become `[package] include_dirs`.

## `completions`

```bash
cbld completions zsh
```
