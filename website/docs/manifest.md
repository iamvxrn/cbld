# The pkgline.toml Manifest

cbld reads a `pkgline.toml` at the root of your project. No build scripts needed.

## Example

```toml
[package]
name = "my_app"
version = "1.0.0"
language = "cpp"
executable = "my_app"
```

## Fields

| Field | Description |
|---|---|
| `name` | Package name |
| `version` | SemVer version string |
| `language` | `c` or `cpp` |
| `executable` | Output binary name |
