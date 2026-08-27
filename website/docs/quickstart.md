# Quickstart

Get up and running with cbld in a few steps.

## 1. Create a project

```bash
cbld init my-app
cd my-app
```

This generates `cbld.toml` and a `src/main.cpp` hello-world (use `--c` for C, `--lib` for a library).

## 2. Build and run

```bash
cbld run
```

Behind the scenes, cbld:

1. Reads `cbld.toml`.
2. Resolves declared dependencies (Git checkouts in `~/.cbld/cache`).
3. Compiles sources with Clang.
4. Links and runs the binary (`cbld run`).

## 3. Depend on an upstream library

Overlay recipes ship for a few trees that have no `cbld.toml`. Clone is unmodified. See [Packages](/packages).

```toml
[dependencies]
"gh:nlohmann/json" = "3.11.3"
```

```cpp
#include <nlohmann/json.hpp>
#include <iostream>

int main() {
    nlohmann::json j = {{"hello", "cbld"}};
    std::cout << j.dump() << std::endl;
}
```

```bash
cbld run
```

## 4. Check your toolchain

```bash
cbld doctor
```

Verifies `clang`, `ar`, headers, and lists the builtin overlay recipes.
