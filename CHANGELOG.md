# Changelog

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
