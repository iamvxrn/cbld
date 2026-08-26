# Changelog

## [0.3.1] - 2026-08-26

Correctness and docs: the 0.3.0 graph, installers, and site had drifted.

- Resolve and build transitive `cbld.toml` dependencies; pass `{ features }` into the dep being compiled
- `cbld sync` still requires `CBLD_LIBS_URL` (no default registry)
- Windows locates `~/.cbld` via `USERPROFILE` when `HOME` is unset
- `cbld doctor` accepts `llvm-ar` / `lib.exe`, not only GNU `ar`
- `cbld migrate` writes `[package] include_dirs` instead of profile `extra_flags`
- Install fallback tags aligned to `v0.3.1`; Linux aarch64 release asset; `uninstall.ps1`
- Docs match `cbld.toml` / CLI (workspace, features-as-defines, architecture page)

## [0.3.0] - 2026-08-25

Package index sync, migrations, diagnostics, and documentation site.

- `sync`, `migrate`, `vendor`, `doctor`, and shell completions
- Global build cache, sanitizer flags, and `--json` build output
- VitePress documentation site and honest CLI/manifest docs

## [0.2.0] - 2026-07-28

CLI expansion, dependency resolution, and release tooling.

- `run`, `init`, `check`, and `update` commands
- Git dependency resolution and `cbld.lock`
- Install scripts for Unix and Windows
- CI for Linux and Windows, nightly builds, and tagged releases

## [0.1.0] - 2026-06-15

Clang-based build tool for single-package C/C++ projects with a fixed `src/` layout.

- `cbld build` with parallel compilation and linking
- `cbld.toml` manifest parsing
- `compile_commands.json` generation
