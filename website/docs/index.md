# cbld

A build engine for C and C++ — strict project layout, Clang under the hood, reproducible builds.

<div class="cbld-install">
  <span class="prompt">$</span>
  <span>curl -fsSL https://cbld.pages.dev/install.sh | sh</span>
  <button onclick="navigator.clipboard.writeText('curl -fsSL https://cbld.pages.dev/install.sh | sh')">Copy</button>
</div>

[Quickstart](/quickstart) · [Other install options](/install) · [GitHub](https://github.com/iamvxrn/cbld)

<div class="cbld-grid">
  <section>
    <h3>Declarative</h3>
    <p>One <code>cbld.toml</code> at the repo root. No Makefiles, no CMake scripts, no build glue.</p>
  </section>
  <section>
    <h3>Clang-first</h3>
    <p>Compile, analyze, and link through LLVM/Clang with the same flags on Linux, macOS, and Windows.</p>
  </section>
  <section>
    <h3>Cached builds</h3>
    <p>Static libraries are fingerprinted and reused from <code>~/.cbld/cache</code> across checkouts.</p>
  </section>
  <section>
    <h3>Vendored deps</h3>
    <p>Git dependencies pinned in <code>cbld.lock</code>; <code>cbld vendor</code> copies them into <code>third_party/</code>.</p>
  </section>
</div>

## Try it

```bash
cbld init my-app && cd my-app
cbld run
```

## What a package looks like

```toml
# cbld.toml
[package]
name = "my_app"
version = "0.1.0"

[profile.cpp]
standard = "c++20"
optimization = "2"
warnings = ["all", "extra"]
```

```
my-app/
├── cbld.toml
├── include/        # optional public headers
└── src/
    └── main.cpp
```
