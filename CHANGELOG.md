# Changelog

## [Unreleased]

Future changes.

## [0.5.2] - 2026-09-02

- Sync the official `iamvxrn/cbld` package index by default; `CBLD_LIBS_URL` overrides it for private or forked indexes.
- Refresh the package index during every `cbld doctor` run and report its result.

## [0.5.1] - 2026-08-31

- C++20 module scanning ignores comments and raw string literals.
- Cache `pkg-config` queries within each build, check, lint, or workspace request.

## [0.5.0] - 2026-08-31

- `cbld test`: discover, build, run, and parse C/C++ test suites with JSON output.
- `cbld bench`: discover and run Google Benchmark-compatible binaries with normalized JSON results.
- Package-local C++20 modules with BMI generation, import ordering, and cycle detection.
- Cross-compilation target presets with `[target.<triple>] sysroot` and isolated output directories.
- ASCII-only vibrant CLI output and updated documentation.

## [0.4.0] - 2026-08-27

Overlay recipes for upstream trees that have no `cbld.toml`, plus header-only packages.

- `[package] kind = "header"` — include-only dependency: no `.a`, just `-I`
- Overlay recipes in the package index (builtin `registry/cbld-libs.toml`); clone upstream as-is; a `cbld.toml` in the clone always wins
- Recipes shipped for `gh:nlohmann/json`, `gh:DaveGamble/cJSON`, `gh:fmtlib/fmt` — listed by `cbld doctor` and on the Packages docs page. CI builds previous pins (`3.11.3` / `1.7.18` / `11.2.0`) and current tags (`3.12.0` / `1.7.19` / `12.2.0`)
- Link dependency archives into the consumer executable (compiled libs actually resolve at link time)
- `cbld fmt` / `cbld fmt --check` — clang-format over sources and public headers (`.clang-format` if present, else LLVM style)
- `cbld lint` / `cbld lint --deny-warnings` — clang-tidy (clippy analog); `--deny-warnings` is `-D warnings`. `cbld check` remains Clang `--analyze`
- `cbld check` / `fmt` / `lint` at a workspace root walk every `[workspace] members` entry
- CI: `cargo fmt --check` and `clippy -D warnings`
- Changelog published on the documentation site
- `cbld sync` still requires `CBLD_LIBS_URL`; the three builtin recipes work without it. Index file may be TOML or the legacy `shorthand <url>` lines
- Install fallback tags aligned to `v0.4.0`

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
