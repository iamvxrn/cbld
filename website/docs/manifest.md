# The cbld.toml Manifest

cbld reads a `cbld.toml` at the root of your project.

## Example (executable)

```toml
[package]
name = "my_app"
version = "0.1.0"
description = "optional"
authors = ["you"]

[profile.cpp]
standard = "c++20"
optimization = "2"
warnings = ["all", "extra"]
rtti = false
exceptions = true

[dependencies]
"gh:owner/repo" = "1.2.3"
"gh:owner/other" = { version = "2.0", features = ["ssl"], tag = "v2.0.1" }

[target.aarch64-unknown-linux-gnu]
sysroot = "toolchains/aarch64-sysroot"
```

`cbld init` scaffolds a minimal manifest plus `src/main.cpp` (or `main.c`, library variants with `--lib` / `--c`). New packages start at `version = "0.1.0"`.

## `[package]` fields

| Field | Description |
|---|---|
| `name` | Package name |
| `version` | Version string (lockfile metadata; not cbld's own version) |
| `description` | Optional free text |
| `authors` | Optional list of strings |
| `toolchain` | Optional pinned compiler, e.g. `clang-18.1` |
| `target` | Optional cross-compile triple for `--target=` |
| `source_dir` | Source scan root (default `src`) |
| `include_dirs` | Extra `-I` paths relative to package root |
| `defines` | Project-wide `-D` defines for C and C++ |
| `libs` | System libraries passed as `-l` when linking an executable (`pthread`, `m`, …) |
| `ignore_warnings` | Inject `-w` for all translation units |
| `kind` | `bin` / `lib` when the entry file name does not imply the artifact type; `header` for include-only (no archive) |
| `include` / `exclude` | Glob patterns to narrow the source scan |

## Target presets

`[target.<triple>]` supplies the sysroot for a selected target. The triple is
selected by `--target` first, then by `[package] target`. Relative sysroots are
resolved from the package root and must point to an existing directory.

```toml
[package]
target = "aarch64-unknown-linux-gnu"

[target.aarch64-unknown-linux-gnu]
sysroot = "toolchains/aarch64-sysroot"
```

Cross-target artifacts are isolated under `target/<triple>/debug` or
`target/<triple>/release`. `cbld run`, `cbld test`, and `cbld bench` refuse to
execute a cross-compiled binary on the host.

## C++20 modules

Package-local standard C++20 modules use `.cppm`, `.ccm`, `.cxxm`, `.c++m`,
`.ixx`, or `.mxx`. `cbld` discovers `export module`, `module`, and `import`
declarations, compiles module interfaces to BMI files, and orders importers
after their producers. Module dependencies from external packages are not
resolved as BMI files yet.

## Profiles

- `[profile.c]` / `[profile.cpp]`: `standard`, `optimization`, `warnings`, `defines`, `sanitizers`, `lto`, `extra_flags`.
- `[profile.cpp]` only: `rtti`, `exceptions`.

## Dependencies and features

- `[dependencies]` — keys like `gh:owner/repo`. Value is a version string or `{ version, features, tag }`. `tag` overrides the git tag when it differs from `version`. `features` are passed into that dependency when compiling it.
- Upstream trees without a `cbld.toml` can still build when an **overlay recipe** exists. The official list is on [Packages](/packages). The clone is unmodified; a `cbld.toml` in the clone always wins over the recipe.
- `[features]` — named groups that expand to extra `-DCBLD_FEATURE_<NAME>` defines. They do **not** turn dependencies on or off.

## Workspace

```toml
[workspace]
members = ["crates/app", "crates/lib"]
```

Each member is a full package with its own `cbld.toml`. `cbld build` at the workspace root builds members in list order.

## Lockfile

`cbld.lock` is written by `cbld build` and `cbld update`. It pins git SHAs for the whole graph, including transitives.
