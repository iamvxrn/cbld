# Installation

## Unix (Linux / macOS)

```bash
curl -fsSL https://cbld.pages.dev/install.sh | sh
```

The binary is installed to `~/.local/bin/cbld`. Add that directory to `PATH` if `cbld doctor` cannot find the command:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Release assets exist for `x86_64` and `aarch64` on Linux and macOS.

Uninstall:

```bash
curl -fsSL https://cbld.pages.dev/uninstall.sh | sh
```

## Windows

```powershell
Invoke-Expression (Invoke-WebRequest -Uri "https://cbld.pages.dev/install.ps1" -UseBasicParsing).Content
```

The binary is installed to `%USERPROFILE%\.local\bin\cbld.exe`. Uninstall:

```powershell
Invoke-Expression (Invoke-WebRequest -Uri "https://cbld.pages.dev/uninstall.ps1" -UseBasicParsing).Content
```

## Check the toolchain

```bash
cbld doctor
```

`doctor` requires Clang. It also lists builtin overlay recipes (see [Packages](/packages)). `cbld build` can fall back to `gcc`, `tcc`, or `zig cc` if Clang is missing, but that path is not what doctor validates.

## Build from source

```bash
git clone https://github.com/iamvxrn/cbld.git
cd cbld
cargo build --release
```
