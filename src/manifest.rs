//! Manifest (`cbld.toml`) and lockfile (`cbld.lock`) data model.
//!
//! These are plain serde structures. We lean on serde defaults and a couple of
//! small custom deserializers (for the `version | { version, features }`
//! shorthand) instead of building a parser by hand. Validation that needs to
//! touch the filesystem lives in the resolver/engine, not here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::error::{CbldError, IoPathExt, Result};

// ---------------------------------------------------------------------------
// cbld.toml
// ---------------------------------------------------------------------------

/// The fully parsed `cbld.toml` manifest.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Manifest {
    /// Optional workspace declaration.
    #[serde(default)]
    pub workspace: Option<Workspace>,

    /// The package metadata. Required for buildable packages.
    pub package: Option<Package>,

    /// Feature flags: name -> list of features it implies.
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,

    /// Compiler profiles, keyed by language (`c` and/or `cpp`).
    #[serde(default)]
    pub profile: Profiles,

    /// Dependency table. Keys are `gh:user/lib` shorthands.
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
}

/// `[workspace]` table.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Workspace {
    #[serde(default)]
    pub members: Vec<String>,
}

/// `[package]` table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    /// Optional pinned toolchain, e.g. `"clang-18.1"`. When set, `cbld doctor`
    /// and the pre-build phase of `cbld build` invoke the active compiler and
    /// abort the build if its reported version doesn't match.
    #[serde(default)]
    pub toolchain: Option<String>,
    /// Optional cross-compilation target triple, e.g.
    /// `"aarch64-unknown-linux-gnu"`. Passed to clang as `--target=<triple>`
    /// during both compilation and linking. `cbld build --target <triple>`
    /// overrides this at the CLI (see `effective_target` in `main.rs`).
    /// Unset by default — a native build never sees a `--target` flag.
    #[serde(default)]
    pub target: Option<String>,

    // --- Legacy layout knobs ---------------------------------------------
    // The four fields below exist so non-conforming C/C++ trees can
    // be built without restructuring. Every default reproduces the
    // original behavior exactly: `source_dir = "src"`, no extra includes, no extra
    // defines, warnings untouched.
    /// The directory (relative to the package root) that holds the sources to
    /// scan. Defaults to `"src"`, so an existing manifest behaves identically.
    /// `cbld build --from <path>` overrides this at the CLI (see
    /// `effective_source_dir` in `main.rs`).
    #[serde(default = "default_source_dir")]
    pub source_dir: String,
    /// Extra header search directories (relative to the package root) added as
    /// `-I<path>` to every translation unit, on top of the package's own
    /// `include/` and any dependency headers. For legacy layouts that keep
    /// public headers somewhere other than `include/`.
    #[serde(default)]
    pub include_dirs: Vec<String>,
    /// Project-wide preprocessor defines (`NAME` or `NAME=VALUE`) applied to
    /// *both* C and C++ translation units as `-D<entry>`. This is additive to
    /// the per-language `[profile.c]`/`[profile.cpp]` `defines`.
    #[serde(default)]
    pub defines: Vec<String>,
    /// System libraries passed to the linker as `-l<name>` when linking an
    /// executable (e.g. `pthread`, `m`). Ignored for static libraries.
    #[serde(default)]
    pub libs: Vec<String>,
    /// pkg-config packages to query for cflags/libs (e.g. `openssl`, `zlib`).
    /// Cflags are injected into every TU, libs into the final link. Requires
    /// `pkg-config` on PATH (`cbld doctor` checks it).
    #[serde(default)]
    pub pkg_config: Vec<String>,
    /// Suppress all compiler warnings by injecting `-w`. A blunt escape hatch
    /// for building noisy legacy code you don't own. `cbld build
    /// --ignore-warnings` turns this on from the CLI regardless of the
    /// manifest.
    #[serde(default)]
    pub ignore_warnings: bool,

    // --- Artifact kind and scan globs ------------------------------------
    /// Artifact kind: `"bin"` (executable), `"lib"` (static library),
    /// `"shared"` (shared library), or `"header"` (include-only, no archive).
    /// When set to `bin`/`lib`/`shared`, cbld no longer requires a
    /// canonically-named `main.*`/`lib.*` entry file — real libraries whose
    /// sources are named `cJSON.c` or `format.cc` build as-is. `header` skips
    /// compilation entirely. Unset (the default) keeps the strict behavior:
    /// the entry file's name decides the kind. Accepts `bin`/`exe`/`executable`,
    /// `lib`/`library`/`static`, `shared`/`cdylib`/`dylib`, and
    /// `header`/`header-only` (see `Crate::parse`).
    #[serde(default)]
    pub kind: Option<String>,
    /// Glob patterns (relative to `source_dir`) that restrict the scan: when
    /// non-empty, only matching files are compiled. Default `[]` = scan
    /// everything. Syntax: `*`, `?`, and `**` (see `glob.rs`).
    #[serde(default)]
    pub include: Vec<String>,
    /// Glob patterns (relative to `source_dir`) pruned from the scan, so a
    /// vendored repo's `tests/`, `examples/`, and fuzzers don't get compiled
    /// into your library. Default `[]`. Applied after `include`.
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// `[profile.c]` and `[profile.cpp]`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Profiles {
    #[serde(default)]
    pub c: Option<CProfile>,
    #[serde(default)]
    pub cpp: Option<CppProfile>,
}

/// Compiler configuration specific to C.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CProfile {
    /// e.g. "c11", "c17", "c2x".
    #[serde(default = "default_c_standard")]
    pub standard: String,
    /// Warning groups: "all", "extra", "error", "pedantic", ...
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Optimization level as a string: "0".."3", "s", "z", "g", "fast".
    #[serde(default = "default_opt")]
    pub optimization: String,
    /// Extra raw flags appended verbatim (escape hatch, normally empty).
    #[serde(default)]
    pub extra_flags: Vec<String>,
    /// Preprocessor defines: NAME or NAME=VALUE.
    #[serde(default)]
    pub defines: Vec<String>,
    /// Clang sanitizers to instrument the build with, e.g. `["address",
    /// "undefined"]`. Default: `[]` (no manifest, or an absent key, produces
    /// zero instrumentation — identical to a v0.3.0 manifest).
    #[serde(default)]
    pub sanitizers: Vec<String>,
    /// Link-Time Optimization. Default: `false`.
    #[serde(default)]
    pub lto: bool,
}

impl Default for CProfile {
    fn default() -> Self {
        CProfile {
            standard: default_c_standard(),
            warnings: Vec::new(),
            optimization: default_opt(),
            extra_flags: Vec::new(),
            defines: Vec::new(),
            sanitizers: Vec::new(),
            lto: false,
        }
    }
}

/// Compiler configuration specific to C++.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CppProfile {
    /// e.g. "c++17", "c++20", "c++23".
    #[serde(default = "default_cpp_standard")]
    pub standard: String,
    /// Enable RTTI. Defaults to true (Clang default).
    #[serde(default = "default_true")]
    pub rtti: bool,
    /// Enable exceptions. Defaults to true (Clang default).
    #[serde(default = "default_true")]
    pub exceptions: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default = "default_opt")]
    pub optimization: String,
    #[serde(default)]
    pub extra_flags: Vec<String>,
    #[serde(default)]
    pub defines: Vec<String>,
    /// Clang sanitizers to instrument the build with, e.g. `["address",
    /// "undefined"]`. Default: `[]` (no manifest, or an absent key, produces
    /// zero instrumentation — identical to a v0.3.0 manifest).
    #[serde(default)]
    pub sanitizers: Vec<String>,
    /// Link-Time Optimization. Default: `false`.
    #[serde(default)]
    pub lto: bool,
}

impl Default for CppProfile {
    fn default() -> Self {
        CppProfile {
            standard: default_cpp_standard(),
            rtti: default_true(),
            exceptions: default_true(),
            warnings: Vec::new(),
            optimization: default_opt(),
            extra_flags: Vec::new(),
            defines: Vec::new(),
            sanitizers: Vec::new(),
            lto: false,
        }
    }
}

/// A parsed `[package] toolchain` pin, e.g. `"clang-18.1"` -> `{ compiler:
/// "clang", version: "18.1" }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainSpec {
    pub compiler: String,
    pub version: String,
}

impl ToolchainSpec {
    /// Parse a `<compiler>-<version>` string. The split is on the first `-`,
    /// so `"clang-18.1"` separates into `"clang"` and `"18.1"`.
    pub fn parse(raw: &str) -> Result<ToolchainSpec> {
        let (compiler, version) = raw.split_once('-').ok_or_else(|| {
            CbldError::Config(format!(
                "invalid toolchain spec '{raw}' (expected '<compiler>-<version>', e.g. 'clang-18.1')"
            ))
        })?;
        if compiler.is_empty() || version.is_empty() {
            return Err(CbldError::Config(format!(
                "invalid toolchain spec '{raw}' (expected '<compiler>-<version>', e.g. 'clang-18.1')"
            )));
        }
        Ok(ToolchainSpec {
            compiler: compiler.to_string(),
            version: version.to_string(),
        })
    }

    /// Invoke the pinned compiler and confirm its reported version matches —
    /// as a dotted-prefix match, so a manifest pin of `"18.1"` accepts any
    /// installed `"18.1.x"` but rejects `"17.x"` or `"19.x"`.
    pub fn validate(&self) -> Result<()> {
        let output = Command::new(&self.compiler)
            .arg("--version")
            .output()
            .map_err(|source| CbldError::CommandSpawn {
                program: self.compiler.clone(),
                source,
            })?;
        if !output.status.success() {
            return Err(CbldError::Config(format!(
                "toolchain check failed: '{} --version' did not exit successfully",
                self.compiler
            )));
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let detected = extract_compiler_version(&text).ok_or_else(|| {
            CbldError::Config(format!(
                "could not determine '{}' version from its --version output",
                self.compiler
            ))
        })?;

        let matches =
            detected == self.version || detected.starts_with(&format!("{}.", self.version));
        if !matches {
            return Err(CbldError::Config(format!(
                "environment unvalidated: manifest pins toolchain '{}-{}' but found '{} {}' \
                 (run `cbld doctor` for details)",
                self.compiler, self.version, self.compiler, detected
            )));
        }
        Ok(())
    }
}

/// Pull the first dotted version-looking token out of a compiler's
/// `--version` first line, e.g. `"clang version 18.1.3"` -> `"18.1.3"`.
fn extract_compiler_version(output: &str) -> Option<String> {
    let first_line = output.lines().next()?;
    first_line.split_whitespace().find_map(|tok| {
        let cleaned = tok.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        let first = cleaned.chars().next()?;
        if first.is_ascii_digit() && cleaned.contains('.') {
            Some(cleaned.to_string())
        } else {
            None
        }
    })
}

fn default_source_dir() -> String {
    "src".to_string()
}
fn default_c_standard() -> String {
    "c17".to_string()
}
fn default_cpp_standard() -> String {
    "c++20".to_string()
}
fn default_opt() -> String {
    "0".to_string()
}
fn default_true() -> bool {
    true
}

/// A dependency value. Supports both the bare-string and table forms:
///
/// ```toml
/// "gh:user/http_parser" = "1.5"
/// "gh:another/ssl"       = { version = "2.1", features = ["ssl"] }
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct Dependency {
    pub version: String,
    pub features: Vec<String>,
    /// Optional explicit branch/tag override; normally the version is the tag.
    pub tag: Option<String>,
}

impl<'de> Deserialize<'de> for Dependency {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Intermediate untagged enum: a string, or a detailed table.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Simple(String),
            Detailed {
                version: String,
                #[serde(default)]
                features: Vec<String>,
                #[serde(default)]
                tag: Option<String>,
            },
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(match raw {
            Raw::Simple(version) => Dependency {
                version,
                features: Vec::new(),
                tag: None,
            },
            Raw::Detailed {
                version,
                features,
                tag,
            } => Dependency {
                version,
                features,
                tag,
            },
        })
    }
}

impl Manifest {
    /// Load and parse a `cbld.toml` from a directory root.
    pub fn load(root: &Path) -> Result<Manifest> {
        let path = root.join("cbld.toml");
        let text = fs::read_to_string(&path).path_ctx(&path)?;
        let manifest: Manifest = toml::from_str(&text).map_err(|e| CbldError::ManifestParse {
            path: path.clone(),
            message: e.to_string(),
        })?;
        Ok(manifest)
    }

    /// Compute the effective set of enabled features given CLI choices.
    ///
    /// Starts from `default` (unless suppressed), unions in the requested
    /// features, then transitively expands using the `[features]` table.
    pub fn resolve_features(&self, requested: &[String], no_default: bool) -> Vec<String> {
        let mut enabled: Vec<String> = Vec::new();
        let mut stack: Vec<String> = Vec::new();

        if !no_default {
            if let Some(defaults) = self.features.get("default") {
                stack.extend(defaults.iter().cloned());
            }
        }
        stack.extend(requested.iter().cloned());

        while let Some(feature) = stack.pop() {
            if enabled.iter().any(|f| f == &feature) {
                continue;
            }
            enabled.push(feature.clone());
            if let Some(implied) = self.features.get(&feature) {
                stack.extend(implied.iter().cloned());
            }
        }

        enabled.sort();
        enabled.dedup();
        enabled
    }

    /// True when this manifest declares a workspace.
    pub fn is_workspace(&self) -> bool {
        self.workspace
            .as_ref()
            .map(|w| !w.members.is_empty())
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// cbld.lock
// ---------------------------------------------------------------------------

/// The complete `cbld.lock` file: a flat list of locked dependencies.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Lockfile {
    #[serde(default, rename = "dependency")]
    pub dependencies: Vec<LockedDependency>,
}

/// One `[[dependency]]` entry in the lockfile.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LockedDependency {
    /// Bare package name (last path segment of the shorthand).
    pub name: String,
    /// Source descriptor, e.g. `git+https://github.com/user/lib`.
    pub source: String,
    /// The exact resolved git commit SHA.
    pub checksum: String,
    /// The version/tag that was requested and resolved.
    pub version: String,
    /// Names of direct dependencies of this package (transitive graph edges).
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl Lockfile {
    /// Load a `cbld.lock` if it exists. Returns `Ok(None)` when absent.
    pub fn load(root: &Path) -> Result<Option<Lockfile>> {
        let path = root.join("cbld.lock");
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).path_ctx(&path)?;
        let lock: Lockfile = toml::from_str(&text).map_err(|e| CbldError::LockParse {
            path: path.clone(),
            message: e.to_string(),
        })?;
        Ok(Some(lock))
    }

    /// Serialize and atomically write the lockfile to the project root.
    pub fn save(&self, root: &Path) -> Result<()> {
        let path = root.join("cbld.lock");
        let mut sorted = self.clone();
        sorted.dependencies.sort_by(|a, b| a.name.cmp(&b.name));
        let header = "# This file is auto-generated by cbld.\n\
                      # It records exact resolved versions for reproducible builds.\n\
                      # Do not edit by hand; run `cbld update` to regenerate.\n\n";
        let body =
            toml::to_string_pretty(&sorted).map_err(|e| CbldError::Serialize(e.to_string()))?;
        let tmp = path.with_extension("lock.tmp");
        fs::write(&tmp, format!("{header}{body}")).path_ctx(&tmp)?;
        fs::rename(&tmp, &path).path_ctx(&path)?;
        Ok(())
    }

    /// Look up a locked dependency by package name.
    pub fn get(&self, name: &str) -> Option<&LockedDependency> {
        self.dependencies.iter().find(|d| d.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_source_dir_defaults_to_src_when_absent() {
        // A manifest without source_dir must parse with the
        // canonical default, so legacy support stays fully opt-in.
        let m: Manifest = toml::from_str("[package]\nname = \"p\"\nversion = \"0.1.0\"\n").unwrap();
        let pkg = m.package.unwrap();
        assert_eq!(pkg.source_dir, "src");
        assert!(pkg.include_dirs.is_empty());
        assert!(pkg.defines.is_empty());
        assert!(pkg.libs.is_empty());
        assert!(!pkg.ignore_warnings);
    }

    #[test]
    fn package_parses_legacy_support_fields() {
        let m: Manifest = toml::from_str(
            "[package]\n\
             name = \"p\"\n\
             version = \"0.1.0\"\n\
             source_dir = \"legacy/src\"\n\
             include_dirs = [\"legacy/include\", \"vendor/include\"]\n\
             defines = [\"LEGACY\", \"VERSION=2\"]\n\
             libs = [\"pthread\", \"m\"]\n\
             ignore_warnings = true\n",
        )
        .unwrap();
        let pkg = m.package.unwrap();
        assert_eq!(pkg.source_dir, "legacy/src");
        assert_eq!(pkg.include_dirs, vec!["legacy/include", "vendor/include"]);
        assert_eq!(pkg.defines, vec!["LEGACY", "VERSION=2"]);
        assert_eq!(pkg.libs, vec!["pthread", "m"]);
        assert!(pkg.ignore_warnings);
    }

    #[test]
    fn package_kind_and_scan_globs_parse_and_default() {
        // Absent kind/globs: kind None, no globs.
        let bare: Manifest =
            toml::from_str("[package]\nname = \"p\"\nversion = \"0.1.0\"\n").unwrap();
        let pkg = bare.package.unwrap();
        assert_eq!(pkg.kind, None);
        assert!(pkg.include.is_empty());
        assert!(pkg.exclude.is_empty());

        let m: Manifest = toml::from_str(
            "[package]\n\
             name = \"cjson\"\n\
             version = \"1.7.18\"\n\
             kind = \"lib\"\n\
             include = [\"*.c\"]\n\
             exclude = [\"tests/**\", \"fuzzing/**\"]\n",
        )
        .unwrap();
        let pkg = m.package.unwrap();
        assert_eq!(pkg.kind.as_deref(), Some("lib"));
        assert_eq!(pkg.include, vec!["*.c"]);
        assert_eq!(pkg.exclude, vec!["tests/**", "fuzzing/**"]);
    }

    #[test]
    fn toolchain_spec_parses_compiler_and_version() {
        let spec = ToolchainSpec::parse("clang-18.1").unwrap();
        assert_eq!(spec.compiler, "clang");
        assert_eq!(spec.version, "18.1");
    }

    #[test]
    fn toolchain_spec_rejects_missing_separator() {
        assert!(ToolchainSpec::parse("clang18.1").is_err());
    }

    #[test]
    fn toolchain_spec_rejects_empty_halves() {
        assert!(ToolchainSpec::parse("-18.1").is_err());
        assert!(ToolchainSpec::parse("clang-").is_err());
    }

    #[test]
    fn extract_compiler_version_finds_dotted_token() {
        assert_eq!(
            extract_compiler_version("clang version 18.1.3"),
            Some("18.1.3".to_string())
        );
        assert_eq!(
            extract_compiler_version("Apple clang version 15.0.0 (clang-1500.3.9.4)"),
            Some("15.0.0".to_string())
        );
        assert_eq!(extract_compiler_version("no version here"), None);
    }

    #[test]
    fn toolchain_spec_version_match_is_dotted_prefix() {
        // Exercised indirectly via the documented semantics: "18.1" should be
        // a prefix-match for a detected "18.1.3", but not for "18.10.0" (the
        // separator-aware prefix check avoids a false match across this
        // exact boundary).
        let detected = "18.1.3";
        let pin = "18.1";
        assert!(detected == pin || detected.starts_with(&format!("{pin}.")));

        let detected_other_minor = "18.10.0";
        let pin = "18.1";
        assert!(
            !(detected_other_minor == pin || detected_other_minor.starts_with(&format!("{pin}.")))
        );
    }
}
