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

## `update`

Re-resolve dependencies and rewrite `cbld.lock` (does not touch the package index).

```bash
cbld update
cbld update http_parser
```

The optional argument is the **package name** (last path segment of `gh:owner/repo`).

## `sync`

Download a flat-text package index into `~/.cbld/cbld-libs`.

```bash
export CBLD_LIBS_URL=https://example.com/cbld-libs
cbld sync
```

Requires `CBLD_LIBS_URL`. There is no default public registry. Host `registry/cbld-libs` yourself if you need shorthand aliases beyond `gh:`.

## `doctor`

Diagnose the local toolchain (`clang`, archiver, headers, fetch tools).

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
