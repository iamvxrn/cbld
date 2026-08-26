# Quickstart

Get up and running with cbld in a few steps.

## 1. Create a project

```bash
cbld init my-app
cd my-app
```

This generates `cbld.toml` and a `src/main.cpp` hello-world (use `--c` for C, `--lib` for a library).

## 2. Build and run

```bash
cbld run
```

Behind the scenes, cbld:

1. Reads `cbld.toml`.
2. Resolves declared dependencies (Git checkouts in `~/.cbld/cache`).
3. Compiles sources with Clang.
4. Links and runs the binary (`cbld run`).

## 3. Check your toolchain

```bash
cbld doctor
```

Verifies `clang`, `ar`, and required headers on your system.
