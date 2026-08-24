# Quickstart

Get up and running with cbld in a few steps.

## 1. Create a project

```bash
cbld init my-app
cd my-app
```

This generates a `pkgline.toml` and a `src/main.cpp` with a hello world.

## 2. Build and run

```bash
cbld run
```

Behind the scenes, cbld:
1. Reads your `pkgline.toml` manifest.
2. Resolves any declared dependencies.
3. Compiles all sources with Clang.
4. Runs the resulting binary.

## 3. Check your toolchain

```bash
cbld doctor
```

This verifies that `clang`, `ar`, and the required headers are all present on your system.
