//! Overlay recipes for upstream trees that ship no `cbld.toml`.
//!
//! A recipe tells cbld how to treat a cloned repository: artifact kind,
//! which files to compile, which directories to put on the include path.
//! The clone itself is never rewritten. If the clone already has a
//! `cbld.toml`, that file wins and the recipe is ignored.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::error::{CbldError, IoPathExt, Result};
use crate::manifest::{Manifest, Package};
use crate::resolver;

const BUILTIN: &str = include_str!("../registry/cbld-libs.toml");

/// One overlay recipe, keyed by the consumer's `[dependencies]` shorthand
/// (e.g. `gh:nlohmann/json`).
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct Recipe {
    /// Upstream git URL. When set, this wins over the built-in `gh:` heuristic.
    #[serde(default)]
    pub git: Option<String>,
    /// `[package] kind`: `header` (include-only) or `lib` / `bin`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub source_dir: Option<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub defines: Vec<String>,
    #[serde(default)]
    pub ignore_warnings: bool,
}

impl Recipe {
    /// True when this entry is more than a URL mapping: it can stand in for a
    /// missing `cbld.toml` in the clone.
    pub fn is_overlay(&self) -> bool {
        self.kind.is_some()
            || self.source_dir.is_some()
            || !self.include_dirs.is_empty()
            || !self.include.is_empty()
            || !self.exclude.is_empty()
            || !self.defines.is_empty()
            || self.ignore_warnings
    }

    /// Materialize a synthetic manifest for an overlay checkout.
    pub fn to_manifest(&self, name: &str, version: &str) -> Manifest {
        Manifest {
            package: Some(Package {
                name: name.to_string(),
                version: version.to_string(),
                description: None,
                authors: Vec::new(),
                toolchain: None,
                target: None,
                source_dir: self.source_dir.clone().unwrap_or_else(|| "src".to_string()),
                include_dirs: self.include_dirs.clone(),
                defines: self.defines.clone(),
                ignore_warnings: self.ignore_warnings,
                kind: self.kind.clone(),
                include: self.include.clone(),
                exclude: self.exclude.clone(),
            }),
            ..Manifest::default()
        }
    }
}

/// Parsed package index: shorthand → recipe (URL mapping and/or overlay).
#[derive(Debug, Clone, Default)]
pub struct PackageIndex {
    pub recipes: BTreeMap<String, Recipe>,
}

impl PackageIndex {
    /// Recipes shipped with this binary (`registry/cbld-libs.toml`).
    pub fn builtin() -> PackageIndex {
        PackageIndex::parse(BUILTIN, "builtin recipes").expect("registry/cbld-libs.toml must parse")
    }

    /// Builtin recipes, overlaid by `~/.cbld/cbld-libs` when that file exists.
    pub fn load() -> Result<PackageIndex> {
        let mut idx = Self::builtin();
        if let Ok(home) = resolver::cbld_home() {
            let path = home.join("cbld-libs");
            if path.is_file() {
                let text = fs::read_to_string(&path).path_ctx(&path)?;
                idx.merge(Self::parse(&text, &path.display().to_string())?);
            }
        }
        Ok(idx)
    }

    /// Parse either TOML recipes or the legacy `shorthand <url>` line format.
    pub fn parse(text: &str, origin: &str) -> Result<PackageIndex> {
        if looks_like_toml(text) {
            parse_toml(text, origin)
        } else {
            parse_legacy(text, origin)
        }
    }

    /// Entries in `other` replace the same keys in `self`.
    pub fn merge(&mut self, other: PackageIndex) {
        self.recipes.extend(other.recipes);
    }

    pub fn git_url(&self, shorthand: &str) -> Option<&str> {
        self.recipes.get(shorthand).and_then(|r| r.git.as_deref())
    }

    /// Shorthands that can stand in for a missing `cbld.toml` (sorted).
    pub fn overlay_shorthands(&self) -> Vec<String> {
        self.recipes
            .iter()
            .filter(|(_, r)| r.is_overlay())
            .map(|(k, _)| k.clone())
            .collect()
    }

    #[allow(dead_code)]
    pub fn get(&self, shorthand: &str) -> Option<&Recipe> {
        self.recipes.get(shorthand)
    }

    /// Look up a recipe by shorthand first, then by git URL (vendor path:
    /// the lockfile stores `git+https://...`, not the original shorthand).
    pub fn find(&self, shorthand: &str, git_url: &str) -> Option<(&str, &Recipe)> {
        if let Some((key, r)) = self.recipes.get_key_value(shorthand) {
            return Some((key.as_str(), r));
        }
        let want = normalize_git_url(git_url);
        self.recipes.iter().find_map(|(key, r)| {
            let got = r.git.as_deref().map(normalize_git_url);
            (got.as_deref() == Some(want.as_str())).then_some((key.as_str(), r))
        })
    }

    /// Load the clone's `cbld.toml` when present; otherwise apply an overlay
    /// recipe. Never writes into the clone.
    pub fn effective_manifest(
        &self,
        cache_path: &Path,
        shorthand: &str,
        git_url: &str,
        name: &str,
        version: &str,
    ) -> Result<Manifest> {
        let manifest_path = cache_path.join("cbld.toml");
        if manifest_path.is_file() {
            return Manifest::load(cache_path);
        }
        if let Some((_, recipe)) = self.find(shorthand, git_url) {
            if recipe.is_overlay() {
                return Ok(recipe.to_manifest(name, version));
            }
        }
        Err(CbldError::NotCbldStandard {
            path: cache_path.to_path_buf(),
            reason: format!(
                "missing cbld.toml (and no overlay recipe for '{shorthand}' in the package index)"
            ),
        })
    }
}

fn looks_like_toml(text: &str) -> bool {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .any(|l| l.starts_with('[') || l.contains('='))
}

fn parse_toml(text: &str, origin: &str) -> Result<PackageIndex> {
    let recipes: BTreeMap<String, Recipe> = toml::from_str(text)
        .map_err(|e| CbldError::Resolution(format!("malformed {origin}: {e}")))?;
    Ok(PackageIndex { recipes })
}

fn parse_legacy(text: &str, origin: &str) -> Result<PackageIndex> {
    let mut recipes = BTreeMap::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some(k), Some(u)) => {
                recipes.insert(
                    k.to_string(),
                    Recipe {
                        git: Some(u.to_string()),
                        ..Recipe::default()
                    },
                );
            }
            _ => {
                return Err(CbldError::Resolution(format!(
                    "malformed mapping in {origin} on line {}: '{raw}'",
                    lineno + 1
                )));
            }
        }
    }
    Ok(PackageIndex { recipes })
}

fn normalize_git_url(url: &str) -> String {
    url.trim()
        .trim_start_matches("git+")
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_recipes_cover_the_three_ci_packages() {
        let idx = PackageIndex::builtin();
        let json = idx.get("gh:nlohmann/json").expect("nlohmann/json recipe");
        assert_eq!(json.kind.as_deref(), Some("header"));
        assert!(json.is_overlay());
        let names = idx.overlay_shorthands();
        assert_eq!(names.len(), 3);
        assert!(names.iter().any(|s| s == "gh:nlohmann/json"));

        let cjson = idx.get("gh:DaveGamble/cJSON").expect("cJSON recipe");
        assert_eq!(cjson.kind.as_deref(), Some("lib"));
        assert_eq!(cjson.source_dir.as_deref(), Some("."));
        assert_eq!(cjson.include, vec!["cJSON.c"]);

        let fmt = idx.get("gh:fmtlib/fmt").expect("fmt recipe");
        assert_eq!(fmt.kind.as_deref(), Some("lib"));
        assert_eq!(fmt.include, vec!["format.cc", "os.cc"]);
    }

    #[test]
    fn clone_cbld_toml_wins_over_overlay_recipe() {
        let dir = std::env::temp_dir().join(format!(
            "cbld-overlay-win-{}-{}",
            std::process::id(),
            "toml"
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("cbld.toml"),
            "[package]\nname = \"from-clone\"\nversion = \"9.9.9\"\nkind = \"lib\"\n",
        )
        .unwrap();

        let mut idx = PackageIndex::default();
        idx.recipes.insert(
            "gh:nlohmann/json".into(),
            Recipe {
                kind: Some("header".into()),
                include_dirs: vec!["include".into()],
                git: Some("https://github.com/nlohmann/json.git".into()),
                ..Recipe::default()
            },
        );

        let m = idx
            .effective_manifest(
                &dir,
                "gh:nlohmann/json",
                "https://github.com/nlohmann/json.git",
                "json",
                "3.11.3",
            )
            .unwrap();
        let pkg = m.package.unwrap();
        assert_eq!(pkg.name, "from-clone");
        assert_eq!(pkg.version, "9.9.9");
        assert_eq!(pkg.kind.as_deref(), Some("lib"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlay_fills_in_when_clone_has_no_manifest() {
        let dir = std::env::temp_dir().join(format!(
            "cbld-overlay-miss-{}-{}",
            std::process::id(),
            "none"
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let idx = PackageIndex::builtin();
        let m = idx
            .effective_manifest(
                &dir,
                "gh:nlohmann/json",
                "https://github.com/nlohmann/json.git",
                "json",
                "3.11.3",
            )
            .unwrap();
        let pkg = m.package.unwrap();
        assert_eq!(pkg.name, "json");
        assert_eq!(pkg.version, "3.11.3");
        assert_eq!(pkg.kind.as_deref(), Some("header"));
        assert_eq!(pkg.include_dirs, vec!["include"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recipe_lookup_falls_back_to_git_url() {
        let idx = PackageIndex::builtin();
        let found = idx
            .find("json", "https://github.com/nlohmann/json.git")
            .expect("url match");
        assert_eq!(found.0, "gh:nlohmann/json");
        assert_eq!(found.1.kind.as_deref(), Some("header"));
    }

    #[test]
    fn missing_manifest_without_recipe_is_not_cbld_standard() {
        let dir = std::env::temp_dir().join(format!(
            "cbld-overlay-none-{}-{}",
            std::process::id(),
            "err"
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let idx = PackageIndex::default();
        let err = idx
            .effective_manifest(
                &dir,
                "gh:foo/bar",
                "https://example.com/foo/bar.git",
                "bar",
                "1.0",
            )
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing cbld.toml"), "{msg}");
        assert!(msg.contains("gh:foo/bar"), "{msg}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_line_format_is_url_only_not_an_overlay() {
        let idx = PackageIndex::parse(
            "# comment\ngh:user/lib https://example.com/user/lib.git\n",
            "test",
        )
        .unwrap();
        let r = idx.get("gh:user/lib").unwrap();
        assert_eq!(r.git.as_deref(), Some("https://example.com/user/lib.git"));
        assert!(!r.is_overlay());
    }

    #[test]
    fn user_index_overrides_builtin_keys() {
        let mut idx = PackageIndex::builtin();
        idx.merge(
            PackageIndex::parse(
                "[\"gh:nlohmann/json\"]\ngit = \"https://example.com/fork.git\"\nkind = \"header\"\n",
                "user",
            )
            .unwrap(),
        );
        assert_eq!(
            idx.git_url("gh:nlohmann/json"),
            Some("https://example.com/fork.git")
        );
        assert!(idx.get("gh:fmtlib/fmt").is_some());
    }
}
