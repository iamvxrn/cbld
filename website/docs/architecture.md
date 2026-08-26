# Architecture

cbld is a single Rust binary. It shells out to tools the OS already has (`clang`, `git`, `ar` / `llvm-ar` / `lib.exe`, `curl` or PowerShell) instead of embedding HTTP, VCS, or compiler libraries.

## Layout

A package is a directory with `cbld.toml` and a source tree (default `src/`). Public headers may live in `include/`. Workspaces list members under `[workspace] members`; each member is built as its own package.

## Dependencies

`gh:owner/repo` maps to `https://github.com/owner/repo.git` unless `~/.cbld/cbld-libs` (filled by `cbld sync` + `CBLD_LIBS_URL`) says otherwise. Resolution walks each dependency's own `cbld.toml` and compiles transitives first. Versions are git tags; `cbld.lock` pins commit SHAs.

`cbld vendor` copies the locked graph into `third_party/` for offline builds.

`[features]` on a package expands to `-DCBLD_FEATURE_*` defines. A dependency table `{ version = "1.0", features = ["ssl"] }` passes those names into the dependency's own feature set when compiling it. Features do not enable or disable dependency entries.

## Build cache

Only **static libraries** go in `~/.cbld/cache/prebuilt/{hash}/`. The hash covers sources, compiler flags, and host OS/arch. Executables are project-specific and are not cached globally.

cbld home is `$CBLD_HOME`, else `$HOME/.cbld`, else `%USERPROFILE%\.cbld` on Windows.

## JSON

`cbld build --json` and `cbld doctor --json` use a small in-tree JSON writer (`src/json.rs`). There is no `serde_json` dependency. Other commands accept `--json` but ignore it.

## Archivers

- Linux: `ar rcsD`
- macOS: `llvm-ar rcsD`, then BSD `ar rcs`
- Windows: `llvm-ar rcsD`, then `lib.exe`

`cbld doctor` reports whichever of those is on PATH, not only GNU `ar`.
