# The cbld.toml Manifest

cbld reads a `cbld.toml` at the root of your project.

## Example (executable)

```toml
[package]
name = "my_app"
version = "0.1.0"

[profile.cpp]
standard = "c++20"
optimization = "2"
warnings = ["all", "extra"]

[dependencies]
# gh:user/repo shorthand — resolved via ~/.cbld/cbld-libs or built-in heuristics
```

`cbld init` scaffolds a minimal manifest plus `src/main.cpp` (or `main.c`, library variants with `--lib` / `--c`).

## `[package]` fields

| Field | Description |
|---|---|
| `name` | Package name |
| `version` | Version string (used for lockfile metadata) |
| `toolchain` | Optional pinned compiler, e.g. `clang-18.1` (`cbld doctor` / build checks) |
| `target` | Optional cross-compile triple for `--target=` |
| `source_dir` | Source scan root (default `src`) |
| `include_dirs` | Extra `-I` paths relative to package root |
| `defines` | Project-wide `-D` defines for C and C++ |
| `ignore_warnings` | Inject `-w` for all translation units |
| `kind` | `bin` / `lib` when entry file name does not imply the artifact type |
| `include` / `exclude` | Glob patterns to narrow the source scan |

## Profiles and dependencies

- `[profile.c]` / `[profile.cpp]` — `standard`, `optimization`, `warnings`, `defines`, `sanitizers`, `lto`, `extra_flags`.
- `[dependencies]` — keys like `gh:owner/repo` with version or `{ version, features }` values.
- `[features]` — optional feature flags for conditional dependencies.

Lockfile: `cbld.lock` (written by `cbld update` / resolved during `cbld build`).
