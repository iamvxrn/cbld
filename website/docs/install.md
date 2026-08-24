# Installation

Install cbld using the shell script:

```bash
curl -fsSL https://cbld.pages.dev/install.sh | sh
```

This downloads the binary for your OS and puts it in `~/.local/bin/cbld`.

### Check your PATH

Make sure your shell can find the `cbld` command:

```bash
cbld doctor
```

If doctor says it can't find Clang or your PATH is wrong, add this to your `~/.bashrc` or `~/.zshrc`:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## Build from Source

```bash
git clone https://github.com/iamvxrn/cbld.git
cd cbld
cargo build --release
```
