# CLI Reference

## `build`

Compile the package and its dependencies.

```bash
cbld build
cbld build --release -j 8
cbld build --from vendor/foo --ignore-warnings
```

Notable flags: `--release`, `-o` / `--output`, `-j` / `--jobs`, `--features`, `--trace`, `--target`, `--from`.

## `run`

Build (if needed) and execute the binary.

```bash
cbld run
```

Accepts the same build flags as `build`.

## `init`

Scaffold a new package in the given directory (or current dir).

```bash
cbld init
cbld init my-app --lib --c
```

## `check`

Run Clang's static analyzer without producing object files or linking.

```bash
cbld check
```

## `update`

Re-resolve dependencies and rewrite `cbld.lock` (does not touch the package index).

```bash
cbld update
cbld update gh:owner/repo
```

## `sync`

Download the flat-text package index into `~/.cbld/cbld-libs`.

```bash
export CBLD_LIBS_URL=https://cbld.pages.dev/cbld-libs
cbld sync
```

Requires `CBLD_LIBS_URL` — there is no default public registry yet.

## `doctor`

Diagnose the local toolchain (`clang`, `ar`, headers, fetch tools for sync).

```bash
cbld doctor
cbld doctor --json
```

## `vendor`

Copy every locked dependency into `third_party/` for offline builds.

```bash
cbld vendor
```

## `migrate`

Generate a starter `cbld.toml` from an existing CMake project.

```bash
cbld migrate --from=cmake
```

Only `--from=cmake` is implemented today (reads `CMakeLists.txt`).

## `completions`

Generate shell completions.

```bash
cbld completions zsh
```
