//! Dependency resolution.
//!
//! Responsibilities:
//!   * Translate `gh:user/lib` shorthands into real repository URLs using
//!     overlay recipes (builtin + `~/.cbld/cbld-libs`) and the `gh:` heuristic.
//!   * Clone (or reuse) dependencies in the global cache `~/.cbld/cache/` at a
//!     specific tag, via the system `git` binary (with `curl` as a probe/
//!     fallback for reachability checks).
//!   * Pin the exact commit SHA and produce/consume `cbld.lock`.
//!
//! The resolver shells out to real `git`/`curl` rather than embedding a VCS
//! library — this keeps the binary small and matches cbld's design.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{CbldError, IoPathExt, Result};
use crate::manifest::{Dependency, LockedDependency, Lockfile, Manifest};
use crate::recipe::PackageIndex;

/// Official package-index endpoint. `CBLD_LIBS_URL` may override this for a
/// private or forked registry.
const DEFAULT_LIBS_URL: &str =
    "https://raw.githubusercontent.com/iamvxrn/cbld/main/registry/cbld-libs.toml";

/// A single resolved dependency, ready to be built and recorded.
#[derive(Debug, Clone)]
pub struct ResolvedDep {
    /// Bare package name (last segment of the shorthand path).
    pub name: String,
    /// Original shorthand key, e.g. `gh:user/http_parser`. Retained for
    /// future tooling (e.g. richer `cbld update` diagnostics) that needs to
    /// echo back exactly what the manifest declared.
    #[allow(dead_code)]
    pub shorthand: String,
    /// Concrete repository URL. Retained for future tooling (e.g. `cbld
    /// doctor` connectivity checks) that needs the resolved origin.
    #[allow(dead_code)]
    pub url: String,
    /// `git+<url>` source descriptor for the lockfile.
    pub source: String,
    /// Requested version/tag.
    pub version: String,
    /// Resolved commit SHA.
    pub checksum: String,
    /// Absolute path to the checked-out copy inside the global cache.
    pub cache_path: PathBuf,
    /// Names of this dependency's own direct dependencies.
    pub dependencies: Vec<String>,
    /// Feature names requested by the depending package (`{ features = [...] }`).
    /// Passed into that dependency's own `resolve_features` when compiling it.
    pub features: Vec<String>,
}

/// Owns the cbld home directories and the shorthand mapping table.
pub struct Resolver {
    /// `~/.cbld`
    home: PathBuf,
    /// `~/.cbld/cache`
    cache: PathBuf,
    /// Parsed overlay recipes + URL mappings (builtin, then ~/.cbld/cbld-libs).
    index: PackageIndex,
    verbose: bool,
}

impl Resolver {
    /// Build a resolver, discovering and (if needed) creating the cbld home.
    pub fn new(verbose: bool) -> Result<Resolver> {
        let home = cbld_home()?;
        let cache = home.join("cache");
        fs::create_dir_all(&cache).path_ctx(&cache)?;
        let index = PackageIndex::load()?;
        Ok(Resolver {
            home,
            cache,
            index,
            verbose,
        })
    }

    /// Path to the cbld home directory (`~/.cbld`). Retained for future
    /// tooling (e.g. `cbld doctor`) that needs to report where cbld's state
    /// lives without reaching into private fields.
    #[allow(dead_code)]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Resolve the full dependency graph of a manifest, including transitives.
    ///
    /// Walks each package's own `cbld.toml` `[dependencies]` depth-first and
    /// emits a topological order (dependents after their dependencies) so
    /// `build_dependencies` can compile archives before they are linked.
    /// Cycles are an error. If `lock` is `Some`, known packages are pinned to
    /// the recorded SHAs; packages missing from the lock resolve freshly.
    pub fn resolve_all(
        &self,
        manifest: &Manifest,
        lock: Option<&Lockfile>,
    ) -> Result<Vec<ResolvedDep>> {
        let mut out = Vec::new();
        let mut visiting = HashSet::new();
        let mut done = HashSet::new();
        for (shorthand, dep) in &manifest.dependencies {
            self.resolve_tree(shorthand, dep, lock, &mut visiting, &mut done, &mut out)?;
        }
        Ok(out)
    }

    fn resolve_tree(
        &self,
        shorthand: &str,
        dep: &Dependency,
        lock: Option<&Lockfile>,
        visiting: &mut HashSet<String>,
        done: &mut HashSet<String>,
        out: &mut Vec<ResolvedDep>,
    ) -> Result<()> {
        let name = package_name(shorthand);
        if done.contains(&name) {
            if let Some(existing) = out.iter_mut().find(|r| r.name == name) {
                for f in &dep.features {
                    if !existing.features.contains(f) {
                        existing.features.push(f.clone());
                    }
                }
            }
            return Ok(());
        }
        if visiting.contains(&name) {
            return Err(CbldError::Resolution(format!(
                "cyclic dependency involving '{shorthand}'"
            )));
        }
        visiting.insert(name.clone());

        let locked = lock.and_then(|l| l.get(&name));
        let mut resolved = self.resolve_one(shorthand, dep, locked)?;
        resolved.features = dep.features.clone();

        let sub_manifest = self.effective_manifest(&resolved)?;
        for (child_key, child_dep) in &sub_manifest.dependencies {
            self.resolve_tree(child_key, child_dep, lock, visiting, done, out)?;
        }

        visiting.remove(&name);
        done.insert(name);
        out.push(resolved);
        Ok(())
    }

    /// Resolve a single dependency entry.
    pub(crate) fn resolve_one(
        &self,
        shorthand: &str,
        dep: &Dependency,
        locked: Option<&LockedDependency>,
    ) -> Result<ResolvedDep> {
        let name = package_name(shorthand);
        let url = self.map_shorthand(shorthand)?;
        let source = format!("git+{url}");
        let tag = dep.tag.clone().unwrap_or_else(|| dep.version.clone());

        let dest = self.cache.join(format!("{name}-{tag}"));

        // Ensure the repository is present in the cache at the requested tag.
        self.ensure_cached(&url, &tag, &dest)?;

        // Determine the SHA: trust the lock if present, else read from the
        // freshly checked-out tree.
        let checksum = match locked {
            Some(l) if l.version == dep.version => {
                // Reproducible path: hard-reset the cache to the locked SHA so
                // the on-disk tree matches the lockfile exactly.
                self.checkout_sha(&dest, &l.checksum)?;
                l.checksum.clone()
            }
            _ => self.head_sha(&dest)?,
        };

        // Read the dependency's own (or overlay) manifest to discover
        // transitive edges.
        let sub_deps = self.direct_dependency_names(shorthand, &url, &name, &dep.version, &dest)?;

        Ok(ResolvedDep {
            name,
            shorthand: shorthand.to_string(),
            url,
            source,
            version: dep.version.clone(),
            checksum,
            cache_path: dest,
            dependencies: sub_deps,
            features: dep.features.clone(),
        })
    }

    /// Translate a `gh:user/lib` shorthand into a concrete URL.
    ///
    /// Resolution order:
    ///   1. Overlay / mapping entry in the package index (builtin recipes,
    ///      then `~/.cbld/cbld-libs`).
    ///   2. Built-in heuristic for the `gh:` prefix -> github.com.
    fn map_shorthand(&self, shorthand: &str) -> Result<String> {
        if let Some(url) = self.index.git_url(shorthand) {
            return Ok(url.to_string());
        }
        if let Some(rest) = shorthand.strip_prefix("gh:") {
            if rest.split('/').count() == 2 && !rest.is_empty() {
                return Ok(format!("https://github.com/{rest}.git"));
            }
        }
        Err(CbldError::Resolution(format!(
            "no mapping for '{shorthand}' in {} and it is not a recognized shorthand",
            self.home.join("cbld-libs").display()
        )))
    }

    pub(crate) fn effective_manifest(&self, dep: &ResolvedDep) -> Result<Manifest> {
        self.index.effective_manifest(
            &dep.cache_path,
            &dep.shorthand,
            &dep.url,
            &dep.name,
            &dep.version,
        )
    }

    /// Make sure `dest` contains a clone of `url` checked out at the requested
    /// version's tag.
    ///
    /// `version` is what the manifest wrote (e.g. `"3.12.0"`); the real tag may
    /// be `v`-prefixed (`v3.12.0`) or not, so we try both spellings
    /// (`tag_candidates`). If none of them exist, that is a hard error — cbld
    /// never silently falls back to the default branch ("latest"), because a
    /// build pinned to a version it can't actually find should fail loudly, not
    /// quietly compile whatever HEAD happens to be.
    fn ensure_cached(&self, url: &str, version: &str, dest: &Path) -> Result<()> {
        let candidates = tag_candidates(version);

        if dest.join(".git").is_dir() {
            self.log(&format!("reusing cached {} @ {}", url, version));
            for tag in &candidates {
                // Fetch the specific tag in case the cache predates it.
                self.git(
                    dest.parent().unwrap_or(&self.cache),
                    &[
                        "-C",
                        &dest.to_string_lossy(),
                        "fetch",
                        "--depth",
                        "1",
                        "origin",
                        "tag",
                        tag,
                    ],
                )
                .ok();
                if self.checkout_tag(dest, tag).is_ok() {
                    return Ok(());
                }
            }
            return Err(CbldError::Resolution(format!(
                "cached checkout of {url} has none of the tags {candidates:?} \
                 (requested version '{version}')"
            )));
        }

        // Probe reachability with curl before a potentially slow clone; this
        // produces a friendlier error for typo'd / private URLs.
        self.probe_url(url)?;

        let parent = dest.parent().unwrap_or(&self.cache);

        // Fast path: shallow clone directly at whichever tag spelling exists.
        for tag in &candidates {
            self.log(&format!("cloning {} @ {} -> {}", url, tag, dest.display()));
            if dest.exists() {
                fs::remove_dir_all(dest).path_ctx(dest)?;
            }
            if self
                .git(
                    parent,
                    &[
                        "clone",
                        "--depth",
                        "1",
                        "--branch",
                        tag,
                        url,
                        &dest.to_string_lossy(),
                    ],
                )
                .is_ok()
            {
                return Ok(());
            }
        }

        // Fallback: one full clone (some hosts disallow `--branch <tag>` on a
        // shallow clone, or the tag is annotated oddly), then check out a
        // matching tag. An unmatched version is still a hard error — we never
        // leave the tree parked on the default branch.
        self.log("shallow tagged clone failed; retrying with full clone");
        if dest.exists() {
            fs::remove_dir_all(dest).path_ctx(dest)?;
        }
        self.git(parent, &["clone", url, &dest.to_string_lossy()])?;
        for tag in &candidates {
            if self.checkout_tag(dest, tag).is_ok() {
                return Ok(());
            }
        }
        Err(CbldError::Resolution(format!(
            "none of the tags {candidates:?} exist in {url} \
             (requested version '{version}') — check the version in your manifest"
        )))
    }

    /// `git checkout <tag>` inside a repository.
    fn checkout_tag(&self, repo: &Path, tag: &str) -> Result<()> {
        self.git(
            repo,
            &["-C", &repo.to_string_lossy(), "checkout", "--quiet", tag],
        )?;
        Ok(())
    }

    /// Hard-reset a repository to an exact SHA (reproducible builds).
    fn checkout_sha(&self, repo: &Path, sha: &str) -> Result<()> {
        // Make sure the object exists locally; deepen if this was a shallow clone.
        if self
            .git(
                repo,
                &["-C", &repo.to_string_lossy(), "cat-file", "-e", sha],
            )
            .is_err()
        {
            self.git(
                repo,
                &["-C", &repo.to_string_lossy(), "fetch", "--unshallow"],
            )
            .ok();
        }
        self.git(
            repo,
            &["-C", &repo.to_string_lossy(), "checkout", "--quiet", sha],
        )?;
        Ok(())
    }

    /// Read the current HEAD commit SHA of a checked-out repository.
    fn head_sha(&self, repo: &Path) -> Result<String> {
        let output = run_capture("git", &["-C", &repo.to_string_lossy(), "rev-parse", "HEAD"])?;
        Ok(output.trim().to_string())
    }

    /// Inspect a cached dependency's overlay or `cbld.toml` for its direct deps.
    fn direct_dependency_names(
        &self,
        shorthand: &str,
        git_url: &str,
        name: &str,
        version: &str,
        repo: &Path,
    ) -> Result<Vec<String>> {
        let sub = self
            .index
            .effective_manifest(repo, shorthand, git_url, name, version)?;
        let mut names: Vec<String> = sub.dependencies.keys().map(|k| package_name(k)).collect();
        names.sort();
        Ok(names)
    }

    /// Use curl to verify a remote URL is reachable before cloning.
    fn probe_url(&self, url: &str) -> Result<()> {
        // Strip a trailing `.git` for the HTTP probe; git smart-HTTP serves
        // `<url>/info/refs?service=git-upload-pack`.
        let base = url.trim_end_matches(".git");
        let probe = format!("{base}/info/refs?service=git-upload-pack");
        let result = run_capture(
            "curl",
            &[
                "--silent",
                "--show-error",
                "--head",
                "--location",
                "--max-time",
                "20",
                "--fail",
                &probe,
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(_) => {
                // curl may be unavailable or the host blocks HEAD; don't hard
                // fail here — let the actual git clone be the source of truth.
                self.log(&format!("curl probe inconclusive for {url}; proceeding"));
                Ok(())
            }
        }
    }

    /// Run a git command rooted in `cwd`, mapping failures into CbldError.
    fn git(&self, cwd: &Path, args: &[&str]) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.current_dir(cwd).args(args);
        let output = cmd.output().map_err(|source| CbldError::CommandSpawn {
            program: "git".to_string(),
            source,
        })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(CbldError::CommandFailed {
                program: format!("git {}", args.join(" ")),
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            })
        }
    }

    fn log(&self, msg: &str) {
        if self.verbose {
            eprintln!("  \x1b[2m[resolver]\x1b[0m {msg}");
        }
    }

    /// Refresh `~/.cbld/cbld-libs` from the registry index.
    ///
    /// Accepts TOML overlay recipes or the legacy `shorthand <url>` line
    /// format. Deliberately shells out to whatever native fetch tool the OS
    /// already has — PowerShell's `Invoke-WebRequest` on Windows, `curl`
    /// (falling back to `wget`) elsewhere — instead of linking an HTTP client
    /// crate. That keeps the dependency tree and compile times exactly as
    /// small as the rest of cbld.
    pub fn sync_index(&self, quiet: bool) -> Result<()> {
        let url = package_index_url();
        let dest = self.home.join("cbld-libs");
        let tmp = self.home.join("cbld-libs.tmp");

        if !quiet {
            println!("\x1b[1;32m    Syncing\x1b[0m package index from {url}");
        }
        self.log(&format!("fetching {url} -> {}", tmp.display()));

        fetch_to_file(&url, &tmp)?;
        // Atomic overwrite: the rename is the only visible mutation of the
        // real index file, so a fetch that dies partway never corrupts it.
        fs::rename(&tmp, &dest).path_ctx(&dest)?;

        if !quiet {
            println!(
                "\x1b[1;32m    Updated\x1b[0m package index at {}",
                dest.display()
            );
        }
        Ok(())
    }
}

fn package_index_url() -> String {
    std::env::var("CBLD_LIBS_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LIBS_URL.to_string())
}

/// Fetch `url` into `dest` using only OS-native tools — no HTTP crate.
fn fetch_to_file(url: &str, dest: &Path) -> Result<()> {
    if cfg!(target_os = "windows") {
        return fetch_with_powershell(url, dest);
    }
    fetch_with_curl_or_wget(url, dest)
}

/// Windows path: native `Invoke-WebRequest` via PowerShell.
fn fetch_with_powershell(url: &str, dest: &Path) -> Result<()> {
    let ps = format!(
        "Invoke-WebRequest -Uri '{}' -OutFile '{}'",
        url,
        dest.display()
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps.as_str()])
        .status()
        .map_err(|source| CbldError::CommandSpawn {
            program: "powershell".to_string(),
            source,
        })?;
    if !status.success() {
        return Err(CbldError::CommandFailed {
            program: "powershell".to_string(),
            code: status.code(),
            stderr: format!("Invoke-WebRequest failed for {url}"),
        });
    }
    Ok(())
}

/// Linux/macOS path: `curl` with `wget` as a fail-safe fallback.
fn fetch_with_curl_or_wget(url: &str, dest: &Path) -> Result<()> {
    let dest_str = dest.to_string_lossy().to_string();

    let curl = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--max-time",
            "30",
            "-o",
            dest_str.as_str(),
            url,
        ])
        .status();

    if let Ok(status) = curl {
        if status.success() {
            return Ok(());
        }
    }

    // curl missing or failed — fall back to wget.
    let status = Command::new("wget")
        .args(["--quiet", "--timeout=30", "-O", dest_str.as_str(), url])
        .status()
        .map_err(|source| CbldError::CommandSpawn {
            program: "curl/wget".to_string(),
            source,
        })?;
    if !status.success() {
        return Err(CbldError::CommandFailed {
            program: "wget".to_string(),
            code: status.code(),
            stderr: format!("failed to fetch {url}"),
        });
    }
    Ok(())
}

/// Build a fresh lockfile from a set of resolved dependencies.
pub fn build_lockfile(resolved: &[ResolvedDep]) -> Lockfile {
    let dependencies = resolved
        .iter()
        .map(|r| LockedDependency {
            name: r.name.clone(),
            source: r.source.clone(),
            checksum: r.checksum.clone(),
            version: r.version.clone(),
            dependencies: r.dependencies.clone(),
        })
        .collect();
    Lockfile { dependencies }
}

/// Extract the bare package name from a shorthand like `gh:user/http_parser`.
pub fn package_name(shorthand: &str) -> String {
    shorthand
        .rsplit('/')
        .next()
        .unwrap_or(shorthand)
        .trim_end_matches(".git")
        .to_string()
}

/// Tag spellings to try for a requested version, in order. Manifests usually
/// write a bare `X.Y.Z` while a lot of projects tag `vX.Y.Z` (and occasionally
/// the reverse), so we accept either: the version as written first, then the
/// opposite `v` prefixing. Deduped so a `v`-prefixed request doesn't retry
/// itself.
fn tag_candidates(version: &str) -> Vec<String> {
    let mut out = vec![version.to_string()];
    match version.strip_prefix('v') {
        Some(bare) if !bare.is_empty() => out.push(bare.to_string()),
        _ => out.push(format!("v{version}")),
    }
    out.dedup();
    out
}

/// Resolve `~/.cbld`, honoring `$CBLD_HOME`, then `$HOME`, then
/// `$USERPROFILE` (Windows).
///
/// `pub(crate)` because the global build cache (`hash.rs`) and `doctor`
/// need the same resolution rule — there is exactly one definition of
/// "where is cbld's home directory" in the codebase.
pub(crate) fn cbld_home() -> Result<PathBuf> {
    if let Ok(explicit) = std::env::var("CBLD_HOME") {
        if !explicit.is_empty() {
            return Ok(PathBuf::from(explicit));
        }
    }
    let home = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()))
        .ok_or_else(|| {
            CbldError::Environment(
                "neither HOME nor USERPROFILE is set; cannot locate ~/.cbld (or set CBLD_HOME)"
                    .into(),
            )
        })?;
    Ok(PathBuf::from(home).join(".cbld"))
}

/// Run a command and capture stdout as a String, mapping errors to CbldError.
fn run_capture(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|source| CbldError::CommandSpawn {
            program: program.to_string(),
            source,
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(CbldError::CommandFailed {
            program: format!("{program} {}", args.join(" ")),
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_index_uses_official_url_when_unset() {
        let prev_url = std::env::var("CBLD_LIBS_URL").ok();
        std::env::remove_var("CBLD_LIBS_URL");
        let url = package_index_url();
        assert_eq!(url, DEFAULT_LIBS_URL);
        if let Some(v) = prev_url {
            std::env::set_var("CBLD_LIBS_URL", v);
        } else {
            std::env::remove_var("CBLD_LIBS_URL");
        }
    }

    #[test]
    fn tag_candidates_tries_both_v_prefix_spellings() {
        // Bare version -> also try the v-prefixed tag (the common case that was
        // silently resolving to "latest" before).
        assert_eq!(tag_candidates("3.12.0"), vec!["3.12.0", "v3.12.0"]);
        // Already v-prefixed -> also try the bare tag.
        assert_eq!(tag_candidates("v3.12.0"), vec!["v3.12.0", "3.12.0"]);
        // A lone "v" isn't a prefix worth stripping.
        assert_eq!(tag_candidates("v"), vec!["v", "vv"]);
    }

    #[test]
    fn package_name_takes_last_segment_without_git_suffix() {
        assert_eq!(package_name("gh:iamvxrn/json"), "json");
        assert_eq!(package_name("gh:user/http_parser.git"), "http_parser");
    }

    #[test]
    fn builtin_recipe_maps_nlohmann_json_without_cbld_libs_file() {
        let prev_home = std::env::var("CBLD_HOME").ok();
        let home =
            std::env::temp_dir().join(format!("cbld-builtin-map-{}-{}", std::process::id(), "idx"));
        let _ = fs::remove_dir_all(&home);
        std::env::set_var("CBLD_HOME", &home);

        let resolver = Resolver::new(false).expect("resolver");
        let url = resolver.map_shorthand("gh:nlohmann/json").unwrap();
        assert_eq!(url, "https://github.com/nlohmann/json.git");

        if let Some(v) = prev_home {
            std::env::set_var("CBLD_HOME", v);
        } else {
            std::env::remove_var("CBLD_HOME");
        }
        let _ = fs::remove_dir_all(&home);
    }
}
