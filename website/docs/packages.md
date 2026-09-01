# Packages

cbld ships overlay recipes for these upstream trees in the binary: clone GitHub as-is, no fork. A `cbld.toml` in the clone always wins.

CI builds both the previous pin and the current upstream tag on every change to `src/` or `registry/`.

| Package | Shorthand | Kind | Tested tags | Headers |
|---|---|---|---|---|
| [nlohmann/json](https://github.com/nlohmann/json) | `gh:nlohmann/json` | header (include-only, no `.a`) | `3.11.3`, `3.12.0` | `#include <nlohmann/json.hpp>` |
| [cJSON](https://github.com/DaveGamble/cJSON) | `gh:DaveGamble/cJSON` | lib | `1.7.18`, `1.7.19` | `#include <cJSON.h>` |
| [fmt](https://github.com/fmtlib/fmt) | `gh:fmtlib/fmt` | lib | `11.2.0`, `12.2.0` | `#include <fmt/core.h>` |
| [CLI11](https://github.com/CLIUtils/CLI11) | `gh:CLIUtils/CLI11` | header (include-only, no `.a`) | `2.6.2`, `2.7.2` | `#include <CLI/CLI.hpp>` |
| [cxxopts](https://github.com/jarro2783/cxxopts) | `gh:jarro2783/cxxopts` | header (include-only, no `.a`) | `3.2.1`, `3.3.1` | `#include <cxxopts.hpp>` |
| [doctest](https://github.com/doctest/doctest) | `gh:doctest/doctest` | header (include-only, no `.a`) | `2.4.11`, `2.4.12` | `#include <doctest/doctest.h>` |
| [magic_enum](https://github.com/Neargye/magic_enum) | `gh:Neargye/magic_enum` | header (include-only, no `.a`) | `0.9.7`, `0.9.8` | `#include <magic_enum/magic_enum.hpp>` |
| [GoogleTest](https://github.com/google/googletest) | `gh:google/googletest` | lib | `1.17.0`, `1.18.0` | `#include <gtest/gtest.h>` |

`kind = "header"` only adds `-I`. Compiled libs are archived and linked into your executable.

The recipe is unversioned: any git tag with the same layout works. There is no `nlohmann/json` `3.12.2` tag; `v3.12.0` is current. fmt `11.0.2` does not compile on Clang 22 (consteval).

## Use one

```toml
[package]
name = "app"
version = "0.1.0"

[profile.cpp]
standard = "c++17"

[dependencies]
"gh:nlohmann/json" = "3.12.0"
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

`cbld doctor` lists the same shorthands. Recipes live in [`registry/cbld-libs.toml`](https://github.com/iamvxrn/cbld/blob/main/registry/cbld-libs.toml). The hosted copy is [`/cbld-libs.toml`](/cbld-libs.toml). The legacy line-format index [`/cbld-libs`](/cbld-libs) is still served for `cbld sync`.
