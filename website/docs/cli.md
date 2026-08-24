# CLI Reference

## `build`

Compile the package and its dependencies.

```bash
cbld build
```

## `run`

Build and execute the binary.

```bash
cbld run
```

## `init`

Scaffold a new project with boilerplate.

```bash
cbld init [name]
```

## `check`

Run Clang's static analyzer without compiling.

```bash
cbld check
```

## `doctor`

Diagnose your local toolchain — checks for `clang`, `ar`, headers.

```bash
cbld doctor
```

## `vendor`

Download all dependencies into `third_party/` for offline builds.

```bash
cbld vendor
```

## `migrate`

Generate a `pkgline.toml` from an existing Makefile or CMakeLists.txt.

```bash
cbld migrate
```

## `completions`

Generate shell completions.

```bash
cbld completions zsh
```
