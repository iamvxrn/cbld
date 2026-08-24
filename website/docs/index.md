# cbld in your terminal

cbld is a single Rust CLI for building C and C++ projects — strict layout, Clang integration, and reproducible builds out of the box.

<div style="display: flex; gap: 12px; margin-top: 1.5rem; margin-bottom: 1.5rem; flex-wrap: wrap; align-items: center;">
  <a href="/quickstart" style="background-color: var(--vp-button-brand-bg); color: var(--vp-button-brand-text); padding: 8px 16px; border-radius: 8px; text-decoration: none; font-weight: 600; font-size: 14px; transition: background-color 0.2s;">Quickstart</a>
  <a href="https://github.com/iamvxrn/cbld" style="background-color: var(--vp-button-alt-bg); color: var(--vp-button-alt-text); padding: 8px 16px; border-radius: 8px; text-decoration: none; font-weight: 600; font-size: 14px; border: 1px solid var(--vp-button-alt-border); transition: background-color 0.2s;">GitHub</a>
  
  <div style="background-color: #161618; border: 1px solid #3c3f44; border-radius: 8px; padding: 6px 12px; display: flex; align-items: center; gap: 8px; font-family: monospace; font-size: 13px;">
    <span style="color: #8b949e;">$</span> curl -fsSL https://cbld.pages.dev/install.sh | sh
    <button style="background: #2c2e33; color: #c9d1d9; border: none; padding: 4px 8px; border-radius: 4px; font-size: 11px; cursor: pointer; margin-left: 8px;">Copy</button>
  </div>
</div>

<div style="display: flex; gap: 8px; flex-wrap: wrap; margin-bottom: 2rem;">
  <span style="background-color: #202127; color: #a1a1aa; padding: 4px 10px; border-radius: 12px; font-size: 12px; font-weight: 500;">Clang</span>
  <span style="background-color: #202127; color: #a1a1aa; padding: 4px 10px; border-radius: 12px; font-size: 12px; font-weight: 500;">C/C++</span>
  <span style="background-color: #202127; color: #a1a1aa; padding: 4px 10px; border-radius: 12px; font-size: 12px; font-weight: 500;">Rust</span>
  <span style="background-color: #202127; color: #a1a1aa; padding: 4px 10px; border-radius: 12px; font-size: 12px; font-weight: 500;">Cross-Platform</span>
  <span style="background-color: #202127; color: #a1a1aa; padding: 4px 10px; border-radius: 12px; font-size: 12px; font-weight: 500;">Vendored Dependencies</span>
</div>

[Other install options →](/install)

---

## Try it

```bash
cbld init my-app && cd my-app
cbld run
```
