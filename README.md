# Cbld

A build system for C and C++ projects with a fixed `src/` layout and Clang integration.

[![Documentation](https://img.shields.io/badge/docs-cbld.pages.dev-4a7bc0.svg)](https://cbld.pages.dev)
[![OS Matrix](https://img.shields.io/badge/OS-Linux%20%7C%20macOS%20%7C%20Windows-4a7bc0.svg)](#)

## Features

- Builds C/C++ with Clang (compile driver can fall back to gcc, tcc, zig cc).
- Strict package layout (`cbld.toml`, `src/`, optional `include/`).
- Git dependencies with `cbld.lock` (including transitives); `compile_commands.json` on every build.
- `build`, `run`, `init`, `check`, `update`, `sync`, `migrate`, `vendor`, `doctor`, `completions`.

## Quick Start

### Installation

```bash
curl -fsSL https://cbld.pages.dev/install.sh | sh
```

Windows:

```powershell
Invoke-Expression (Invoke-WebRequest -Uri "https://cbld.pages.dev/install.ps1" -UseBasicParsing).Content
```

### Usage

```bash
cbld init
cbld build
cbld run
cbld check
cbld update
```

## Documentation

[https://cbld.pages.dev](https://cbld.pages.dev)

## License

MIT
