//! Build engine: project layout enforcement, parallel compilation, linking,
//! and human-readable Clang diagnostics.
//!
//! Concurrency uses only the standard library: a pool of native threads pulls
//! `CompileUnit`s off a shared work queue (guarded by a `Mutex`) and reports
//! results back over an `mpsc` channel. No external job-server crate, no async
//! runtime — just `std::thread` + `std::sync`.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use crate::compdb::CompileCommandEntry;
use crate::compiler::{driver_command, CompileUnit, Compiler, Language, LinkCommand};
use crate::error::{CbldError, CompileDiagnostic, IoPathExt, Result};
use crate::hash;
use crate::manifest::{Manifest, Package};
use crate::resolver;
use crate::trace;

/// What kind of artifact a package produces. Normally decided by which entry
/// file the layout contains (`main.*` → executable, `lib.*` → library), but
/// overridable via `[package] kind` for trees whose sources aren't
/// canonically named. `Header` is include-only: no translation units, no
/// archive — the consumer just gets `-I` paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crate {
    Executable,
    Library,
    Shared,
    Header,
}

impl Crate {
    /// Parse a `[package] kind` string. Accepts the common spellings for each.
    pub fn parse(raw: &str) -> Result<Crate> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "bin" | "exe" | "executable" | "binary" => Ok(Crate::Executable),
            "lib" | "library" | "staticlib" | "static" => Ok(Crate::Library),
            "shared" | "sharedlib" | "cdylib" | "dylib" | "so" | "dll" => Ok(Crate::Shared),
            "header" | "headers" | "header-only" | "hdr" => Ok(Crate::Header),
            other => Err(CbldError::Config(format!(
                "unknown [package] kind '{other}' (expected 'bin', 'lib', 'shared', or 'header')"
            ))),
        }
    }
}

/// How to locate and scan a package's sources. Bundles the legacy-support
/// knobs so `discover` doesn't grow a parameter per manifest field.
#[derive(Debug, Clone, Default)]
pub struct ScanConfig {
    /// Sources directory relative to the package root (`[package] source_dir`
    /// or `cbld build --from`); the default `"src"` reproduces strict layout.
    pub source_dir: String,
    /// Explicit artifact kind (`[package] kind`). When set, cbld skips
    /// canonical-entry discovery entirely, so a directory of arbitrarily-named
    /// sources (`cJSON.c`, `format.cc`) builds without a `main.*`/`lib.*` file.
    /// `Header` also skips the `source_dir` requirement — there is nothing
    /// to compile.
    pub kind: Option<Crate>,
    /// `[package] include` globs (relative to `source_dir`): when non-empty,
    /// only matching files are compiled.
    pub include: Vec<String>,
    /// `[package] exclude` globs (relative to `source_dir`): pruned from the
    /// scan so a vendored repo's `tests/`/`examples/`/fuzzers stay out.
    pub exclude: Vec<String>,
}

impl ScanConfig {
    /// The strict default: scan `src/`, infer kind from the entry file, no
    /// globs. Convenient for the few call sites that don't carry a manifest.
    #[allow(dead_code)] // used by tests; real call sites build ScanConfig from the manifest
    pub fn strict(source_dir: &str) -> ScanConfig {
        ScanConfig {
            source_dir: source_dir.to_string(),
            ..ScanConfig::default()
        }
    }
}

/// The discovered, validated layout of a single package.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Package root (the directory that contains `cbld.toml`).
    pub root: PathBuf,
    pub src: PathBuf,
    /// The canonical entry file, when one was found. `None` for a
    /// `kind`-driven package whose sources are arbitrarily named.
    #[allow(dead_code)]
    pub entry: Option<PathBuf>,
    pub entry_language: Language,
    pub crate_kind: Crate,
    /// Scan globs carried from `ScanConfig` so `collect_sources` filters the
    /// tree exactly the way discovery did.
    include: Vec<String>,
    exclude: Vec<String>,
}

impl Layout {
    /// Locate and validate a package's layout.
    ///
    /// The strict path (no `kind`, default `source_dir`, no globs) is
    /// unchanged from earlier releases: the entry point must be a canonically
    /// named `main.<ext>` (executable) or `lib.<ext>` (library), with `main`
    /// winning over `lib`, and the canonical `.cpp`/`.c` tried before the
    /// legacy `.cc`/`.cxx`/`.C` extensions.
    ///
    /// Legacy escape hatches: `source_dir` redirects where cbld
    /// looks; `include`/`exclude` globs narrow the scan; and `kind` removes
    /// the canonical-entry requirement entirely — with `kind` set, cbld
    /// determines the language from the scanned sources and builds them as the
    /// declared artifact, so a real library named `cJSON.c` builds as-is. The
    /// one-language-per-package rule is unchanged throughout.
    pub fn discover(root: &Path, cfg: &ScanConfig) -> Result<Layout> {
        if cfg.kind == Some(Crate::Header) {
            return Ok(Layout {
                root: root.to_path_buf(),
                src: root.join(&cfg.source_dir),
                entry: None,
                entry_language: Language::Cpp,
                crate_kind: Crate::Header,
                include: cfg.include.clone(),
                exclude: cfg.exclude.clone(),
            });
        }

        let src = root.join(&cfg.source_dir);
        if !src.is_dir() {
            return Err(CbldError::LayoutViolation(format!(
                "missing '{}/' source directory under {}",
                cfg.source_dir,
                root.display()
            )));
        }

        // Canonical entry discovery — the backward-compatible fast path. Only
        // consulted when the manifest hasn't already declared `kind`.
        let canonical = if cfg.kind.is_none() {
            find_canonical_entry(&src)
        } else {
            None
        };

        let (entry, crate_kind, entry_language) = match canonical {
            Some((entry, kind, lang)) => (Some(entry), kind, lang),
            None => {
                // No canonical entry: the kind must be declared, and the
                // language is inferred from whatever sources the scan finds.
                let kind = cfg.kind.ok_or_else(|| {
                    CbldError::LayoutViolation(format!(
                        "no entry point found under {sd}/ and no [package] kind declared. \
                         Either add {sd}/main.<ext> or {sd}/lib.<ext>, or set \
                         kind = \"bin\" | \"lib\" | \"header\" in [package] to build a directory of \
                         arbitrarily-named sources (or an include-only tree).",
                        sd = cfg.source_dir
                    ))
                })?;
                let lang = infer_language(&src, &cfg.include, &cfg.exclude)?;
                (None, kind, lang)
            }
        };

        Ok(Layout {
            root: root.to_path_buf(),
            src,
            entry,
            entry_language,
            crate_kind,
            include: cfg.include.clone(),
            exclude: cfg.exclude.clone(),
        })
    }

    /// Verify a directory is a valid cbld-standard package (manifest + layout).
    pub fn assert_cbld_standard(root: &Path, cfg: &ScanConfig) -> Result<Layout> {
        if !root.join("cbld.toml").is_file() {
            return Err(CbldError::NotCbldStandard {
                path: root.to_path_buf(),
                reason: "missing cbld.toml manifest".to_string(),
            });
        }
        Layout::discover(root, cfg).map_err(|e| CbldError::NotCbldStandard {
            path: root.to_path_buf(),
            reason: e.to_string(),
        })
    }

    /// Gather every compilable translation unit under `source_dir`, honoring
    /// the `include`/`exclude` globs and the strict single-language rule: the
    /// package's language dictates which sources are eligible, and finding the
    /// *other* language is an error. A cbld package is single-language.
    pub fn collect_sources(&self) -> Result<Vec<PathBuf>> {
        let scanned = scan_sources(&self.src, &self.include, &self.exclude)?;

        let mut sources = Vec::new();
        let mut foreign = Vec::new();
        for path in scanned {
            match Language::from_extension(&path) {
                Some(l) if l == self.entry_language => sources.push(path),
                Some(_) => foreign.push(path),
                None => {}
            }
        }

        if !foreign.is_empty() {
            let other = match self.entry_language {
                Language::C => "C++",
                Language::Cpp => "C",
            };
            return Err(CbldError::LayoutViolation(format!(
                "strict C/C++ separation violated: this is a {} package but found \
                 {} {} source file(s) (e.g. '{}'). A cbld package is single-language.",
                self.entry_language.label(),
                foreign.len(),
                other,
                foreign[0].display()
            )));
        }

        sources.sort();
        Ok(sources)
    }

    /// Sources plus public headers (`include/` and any headers under
    /// `source_dir`). Used by `cbld fmt` and `cbld lint`, which visit the
    /// files a human edits, not only the translation units clang compiles.
    pub fn collect_format_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        if self.src.is_dir() {
            if self.crate_kind != Crate::Header {
                files.extend(self.collect_sources()?);
            }
            files.extend(scan_headers(&self.src, &self.include, &self.exclude)?);
        }
        let include_dir = self.root.join("include");
        if include_dir.is_dir() {
            files.extend(scan_headers(&include_dir, &[], &[])?);
        }
        files.sort();
        files.dedup();
        if files.is_empty() {
            return Err(CbldError::LayoutViolation(format!(
                "no C/C++ sources or headers to format under {}",
                self.root.display()
            )));
        }
        Ok(files)
    }

    /// A copy of this layout forced to build as a library. Dependencies are
    /// always archived and linked into the consumer regardless of whether they
    /// expose a `main.*` entry of their own. Header-only packages stay
    /// header-only — there is nothing to archive.
    pub fn as_library(&self) -> Layout {
        if self.crate_kind == Crate::Header {
            return self.clone();
        }
        Layout {
            crate_kind: Crate::Library,
            ..self.clone()
        }
    }
}

/// Look for a canonically named entry file directly under `src`.
///
/// Precedence: an executable entry (`main`) wins over a library entry, and
/// within each stem the canonical `.cpp`/`.c` are tried before the legacy
/// extensions. The language is derived from the extension via the single
/// source of truth, `Language::from_extension`, so entry routing and per-unit
/// routing can never disagree (notably `.C` == C++).
///
/// Matching is done against the directory's *actual* filenames with a
/// case-sensitive comparison, not `src.join(name).is_file()`. On a
/// case-insensitive filesystem (Windows, and macOS by default) `main.c` and
/// `main.C` are the same file on disk, so a plain `is_file()` probe for
/// `main.c` would match a real `main.C` and mis-route a C++ entry as C. Reading
/// the real name back preserves its case (both OSes are case-*preserving*), so
/// `.C` stays C++ everywhere.
fn find_canonical_entry(src: &Path) -> Option<(PathBuf, Crate, Language)> {
    const ENTRY_EXTS: [&str; 7] = ["cpp", "c", "cc", "cxx", "C", "c++", "cp"];
    let names: Vec<(String, PathBuf)> = fs::read_dir(src)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(|n| (n.to_string(), e.path())))
        .collect();

    for (stem, kind) in [("main", Crate::Executable), ("lib", Crate::Library)] {
        for ext in ENTRY_EXTS {
            let target = format!("{stem}.{ext}");
            if let Some((_, path)) = names.iter().find(|(name, _)| name == &target) {
                let lang = Language::from_extension(path).expect("ENTRY_EXTS are recognized");
                return Some((path.clone(), kind, lang));
            }
        }
    }
    None
}

/// Determine a `kind`-driven package's language from its (glob-filtered)
/// sources. Exactly one language must be present; both is a single-language
/// violation, and none is a "nothing to build" error.
fn infer_language(src: &Path, include: &[String], exclude: &[String]) -> Result<Language> {
    let scanned = scan_sources(src, include, exclude)?;
    let mut c_file = None;
    let mut cpp_file = None;
    for path in &scanned {
        match Language::from_extension(path) {
            Some(Language::C) => c_file.get_or_insert(path.clone()),
            Some(Language::Cpp) => cpp_file.get_or_insert(path.clone()),
            None => continue,
        };
    }
    match (c_file, cpp_file) {
        (Some(_), Some(cpp)) => Err(CbldError::LayoutViolation(format!(
            "strict C/C++ separation violated: this package mixes C and C++ sources \
             (e.g. '{}'). A cbld package is single-language — split it, or narrow the \
             scan with [package] include/exclude.",
            cpp.display()
        ))),
        (Some(_), None) => Ok(Language::C),
        (None, Some(_)) => Ok(Language::Cpp),
        (None, None) => Err(CbldError::LayoutViolation(format!(
            "no compilable C/C++ sources found under {} (after include/exclude filters)",
            src.display()
        ))),
    }
}

/// Recursively collect every recognized C/C++ source under `src`, honoring the
/// `include`/`exclude` globs. `exclude` prunes whole directories (so an
/// excluded `tests/` is never descended into); `include`, when non-empty,
/// keeps only matching files. Patterns match `/`-separated, `src`-relative
/// paths.
fn scan_sources(src: &Path, include: &[String], exclude: &[String]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    scan_rec(src, src, include, exclude, false, &mut out)?;
    out.sort();
    Ok(out)
}

/// Headers under `dir`, honoring the same include/exclude globs as sources.
fn scan_headers(dir: &Path, include: &[String], exclude: &[String]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    scan_rec(dir, dir, include, exclude, true, &mut out)?;
    out.sort();
    Ok(out)
}

fn scan_rec(
    base: &Path,
    dir: &Path,
    include: &[String],
    exclude: &[String],
    headers: bool,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir).path_ctx(dir)? {
        let path = entry.path_ctx(dir)?.path();
        let rel = path.strip_prefix(base).unwrap_or(&path);
        let rel_glob = rel.to_string_lossy().replace('\\', "/");

        if crate::glob::matches_any(exclude, &rel_glob) {
            continue; // prunes directories as well as files
        }
        if path.is_dir() {
            scan_rec(base, &path, include, exclude, headers, out)?;
        } else {
            let wanted = if headers {
                Language::is_header(&path)
            } else {
                Language::from_extension(&path).is_some()
            };
            if wanted && (include.is_empty() || crate::glob::matches_any(include, &rel_glob)) {
                out.push(path);
            }
        }
    }
    Ok(())
}

/// Outcome of compiling one translation unit, sent back over the channel.
struct UnitResult {
    source: PathBuf,
    success: bool,
    diagnostics: Vec<Diagnostic>,
    raw_stderr: String,
}

/// The artifact produced by [`Engine::build_package`], plus whether it came
/// from the global build cache instead of a fresh compile.
pub struct BuiltArtifact {
    pub path: PathBuf,
    pub cache_hit: bool,
    /// One `compile_commands.json` entry per translation unit in this
    /// package — populated regardless of `cache_hit`, since the compile
    /// flags are fully determined without actually invoking the compiler.
    pub compile_commands: Vec<CompileCommandEntry>,
}

/// Where [`Engine::build_package`] writes the artifact. Bundled so the
/// method stays under clippy's argument-count lint without losing the
/// distinct dest knobs (`target/debug` vs `release`, `-o`, archives).
#[derive(Clone, Copy)]
pub struct BuildDest<'a> {
    pub target_dir: &'a Path,
    pub output_name: Option<&'a str>,
    pub release: bool,
}

/// Top-level build orchestrator.
pub struct Engine {
    jobs: usize,
    verbose: bool,
    quiet: bool,
    /// When true, suppress human-readable progress/diagnostic text — the
    /// caller is rendering a single structured `--json` payload instead.
    json: bool,
    /// When true, aggregate this package's `-ftime-trace` output into
    /// `cbld_profile.json` and print a bottleneck summary after compiling
    /// (`cbld build --trace`). Has no effect if the compiler wasn't also
    /// told to emit `-ftime-trace` (see `Compiler::new`'s `trace` param).
    trace: bool,
}

impl Engine {
    pub fn new(jobs: usize, verbose: bool, quiet: bool, json: bool, trace: bool) -> Engine {
        let jobs = jobs.max(1);
        Engine {
            jobs,
            verbose,
            quiet,
            json,
            trace,
        }
    }

    /// Compile and link a package. Returns the produced artifact's path and
    /// whether it was served from the global build cache.
    ///
    /// Only library artifacts (static archives) participate in the global
    /// cache — see website/docs/architecture.md for why executables, whose
    /// output is project-specific, are out of scope for it.
    pub fn build_package(
        &self,
        layout: &Layout,
        package: &Package,
        compiler: &Compiler,
        dest: BuildDest<'_>,
        dep_archives: &[PathBuf],
    ) -> Result<BuiltArtifact> {
        compiler.validate()?;

        if layout.crate_kind == Crate::Header {
            if !self.quiet {
                println!(
                    "\x1b[1;32m     Header\x1b[0m {} v{} (include-only, no archive)",
                    package.name, package.version
                );
            }
            return Ok(BuiltArtifact {
                path: layout.root.join("include"),
                cache_hit: false,
                compile_commands: Vec::new(),
            });
        }

        let profile_dir = dest
            .target_dir
            .join(if dest.release { "release" } else { "debug" });
        let obj_dir = profile_dir.join("obj").join(&package.name);
        fs::create_dir_all(&obj_dir).path_ctx(&obj_dir)?;

        let sources = layout.collect_sources()?;
        if sources.is_empty() {
            let rel = layout.src.strip_prefix(&layout.root).unwrap_or(&layout.src);
            return Err(CbldError::LayoutViolation(format!(
                "package '{}' has no source files under {}/",
                package.name,
                rel.display()
            )));
        }

        let artifact = artifact_path(
            &profile_dir,
            layout.crate_kind,
            &package.name,
            dest.output_name,
        );

        // Plan every translation unit up front. This is pure argument-vector
        // construction — no filesystem or process work — so a
        // `compile_commands.json` entry exists for every source file
        // regardless of whether the package below turns out to be served
        // from the global cache.
        let cwd = std::env::current_dir().unwrap_or_else(|_| dest.target_dir.to_path_buf());
        let mut units = Vec::with_capacity(sources.len());
        let mut has_cpp = false;
        for src in &sources {
            let obj = object_path(&obj_dir, layout, src);
            let mut unit = compiler.compile_unit(src, &obj)?;
            if layout.crate_kind == Crate::Shared {
                unit.args.push("-fPIC".to_string());
            }
            if unit.language == Language::Cpp {
                has_cpp = true;
            }
            units.push(unit);
        }
        let compile_commands: Vec<CompileCommandEntry> = units
            .iter()
            .map(|u| CompileCommandEntry {
                directory: cwd.clone(),
                file: u.source.clone(),
                arguments: {
                    let mut args: Vec<String> = u
                        .language
                        .driver()
                        .split_whitespace()
                        .map(str::to_string)
                        .collect();
                    args.extend(u.args.clone());
                    args
                },
            })
            .collect();

        // --- Global cache short-circuit ---------------------------------
        // Before spinning up the compile thread-pool, see whether a
        // byte-identical build (same sources, same flags, same target) has
        // already been cached globally under ~/.cbld/cache/prebuilt/{hash}.
        let cache_key = if layout.crate_kind == Crate::Library {
            let fingerprint = compiler.cache_fingerprint(layout.entry_language)?;
            let mut cache_files = sources.clone();
            cache_files.extend(scan_headers(&layout.src, &layout.include, &layout.exclude)?);
            let include_dir = layout.root.join("include");
            if include_dir.is_dir() {
                cache_files.extend(scan_headers(&include_dir, &[], &[])?);
            }
            cache_files.sort();
            cache_files.dedup();
            Some(hash::package_key(&cache_files, &fingerprint)?)
        } else {
            None
        };

        if let Some(key) = &cache_key {
            if let Ok(home) = resolver::cbld_home() {
                if let Some(cached) = hash::lookup(&home, key, &package.name) {
                    if let Some(parent) = artifact.parent() {
                        fs::create_dir_all(parent).path_ctx(parent)?;
                    }
                    fs::copy(&cached, &artifact).path_ctx(&artifact)?;
                    if !self.quiet {
                        println!(
                            "\x1b[1;96m  Cache hit\x1b[0m \x1b[1;97m{}\x1b[0m \x1b[93mv{}\x1b[0m \x1b[2m[{}]\x1b[0m",
                            package.name, package.version, key
                        );
                    }
                    return Ok(BuiltArtifact {
                        path: artifact,
                        cache_hit: true,
                        compile_commands,
                    });
                }
            }
        }

        // Now that we know we're actually compiling, create each object
        // file's parent directory (skipped entirely on the cache hit above).
        for unit in &units {
            if let Some(parent) = unit.object.parent() {
                fs::create_dir_all(parent).path_ctx(parent)?;
            }
        }

        // Per-file incremental: skip TUs whose object is newer than source
        // and all headers in the previous .d depfile.
        let mut to_compile: Vec<CompileUnit> = Vec::new();
        let mut all_objects: Vec<PathBuf> = Vec::with_capacity(units.len());
        let mut skipped = 0usize;
        for mut unit in units {
            let depfile = unit.object.with_extension("d");
            all_objects.push(unit.object.clone());
            if object_is_fresh(&unit.source, &unit.object, &depfile) {
                skipped += 1;
                continue;
            }
            // Generate depfile for next incremental check.
            unit.args.push("-MMD".to_string());
            unit.args.push("-MP".to_string());
            unit.args.push("-MF".to_string());
            unit.args.push(depfile.to_string_lossy().to_string());
            to_compile.push(unit);
        }

        if !self.quiet {
            if skipped > 0 && !to_compile.is_empty() {
                println!(
                    "\x1b[1;95m   Compiling\x1b[0m \x1b[1;97m{}\x1b[0m \x1b[93mv{}\x1b[0m (\x1b[96m{} unit{}\x1b[0m, \x1b[96m{} job{}\x1b[0m, \x1b[92m{} up-to-date\x1b[0m)",
                    package.name,
                    package.version,
                    to_compile.len(),
                    plural(to_compile.len()),
                    self.jobs,
                    plural(self.jobs),
                    skipped
                );
            } else if skipped > 0 && to_compile.is_empty() {
                println!(
                    "\x1b[1;92m   Fresh\x1b[0m \x1b[1;97m{}\x1b[0m \x1b[93mv{}\x1b[0m (\x1b[92m{} up-to-date\x1b[0m)",
                    package.name, package.version, skipped
                );
            } else {
                println!(
                    "\x1b[1;95m   Compiling\x1b[0m \x1b[1;97m{}\x1b[0m \x1b[93mv{}\x1b[0m (\x1b[96m{} unit{}\x1b[0m, \x1b[96m{} job{}\x1b[0m)",
                    package.name,
                    package.version,
                    to_compile.len(),
                    plural(to_compile.len()),
                    self.jobs,
                    plural(self.jobs),
                );
            }
        }

        let started = Instant::now();
        let objects = if to_compile.is_empty() {
            all_objects
        } else {
            let compiled = self.compile_all(to_compile)?;
            // compile_all returns objects for compiled units; merge with all_objects
            // by replacing compiled entries. Simpler: just use all_objects since
            // compiled objects are at same paths.
            let _ = compiled;
            all_objects
        };
        if self.verbose {
            eprintln!(
                "  \x1b[2m[engine]\x1b[0m compiled in {:.2}s",
                started.elapsed().as_secs_f64()
            );
        }

        if self.trace {
            trace::aggregate_and_report(&obj_dir, &profile_dir, self.quiet);
        }

        if let Some(parent) = artifact.parent() {
            fs::create_dir_all(parent).path_ctx(parent)?;
        }

        // Static archives of dependencies are linked only into executables
        // and shared libs. GNU ld / lld are single-pass: dependents must
        // appear before the libraries they need, so the caller collects
        // archives in reverse topological order (see `build_dependencies`).
        let mut link_inputs = objects;
        if layout.crate_kind == Crate::Executable || layout.crate_kind == Crate::Shared {
            link_inputs.extend(dep_archives.iter().cloned());
        }
        let link = compiler.link_command(
            &link_inputs,
            &artifact,
            has_cpp,
            layout.crate_kind == Crate::Library,
            layout.crate_kind == Crate::Shared,
            &package.libs,
        );
        self.run_link(&link, layout.crate_kind)?;

        if !self.quiet {
            let kind = match layout.crate_kind {
                Crate::Executable => "executable",
                Crate::Library => "library",
                Crate::Shared => "shared library",
                Crate::Header => "header",
            };
            println!(
                "\x1b[1;92m    Finished\x1b[0m \x1b[97m{}\x1b[0m \x1b[2m{}\x1b[0m \x1b[90m[{}]\x1b[0m",
                kind,
                artifact.display(),
                if dest.release {
                    "\x1b[93moptimized\x1b[0m"
                } else {
                    "\x1b[96munoptimized + debuginfo\x1b[0m"
                }
            );
        }

        // Populate the global cache for next time. Best-effort: a cache
        // write failure (e.g. an unwritable ~/.cbld) must never fail an
        // otherwise-successful build.
        if let Some(key) = &cache_key {
            if let Ok(home) = resolver::cbld_home() {
                let _ = hash::store(&home, key, &package.name, &artifact);
            }
        }

        Ok(BuiltArtifact {
            path: artifact,
            cache_hit: false,
            compile_commands,
        })
    }

    /// `cbld check`: run Clang's static analyzer (`--analyze`) over every
    /// source file in `layout`, streaming diagnostics to the terminal as
    /// they arrive. Never touches the linker and never produces an object
    /// file or artifact — a pure read of the source tree.
    ///
    /// Unlike `build_package`/`compile_all`, a unit's diagnostics are
    /// printed **regardless of severity or success**: the whole point of
    /// `cbld check` is to surface analyzer findings, not just errors. Only a
    /// unit that clang itself couldn't parse (non-zero exit) counts as a
    /// failure — analyzer findings on an otherwise-clean parse are printed
    /// as warnings and never fail the command, the same way a successful
    /// `cbld build` can still emit compiler warnings without failing.
    pub fn check_package(&self, layout: &Layout, compiler: &Compiler) -> Result<()> {
        let sources = layout.collect_sources()?;
        if sources.is_empty() {
            return Err(CbldError::LayoutViolation(format!(
                "no source files found under {}",
                layout.src.display()
            )));
        }

        let mut units = Vec::with_capacity(sources.len());
        for src in &sources {
            units.push(compiler.analyze_unit(src)?);
        }

        if !self.quiet {
            println!(
                "\x1b[1;96m   Checking\x1b[0m \x1b[96m{} file{}\x1b[0m (\x1b[96m{} job{}\x1b[0m)",
                units.len(),
                plural(units.len()),
                self.jobs,
                plural(self.jobs),
            );
        }

        let total = units.len();
        let queue: Arc<Mutex<VecDeque<CompileUnit>>> = Arc::new(Mutex::new(VecDeque::from(units)));
        let (tx, rx) = mpsc::channel::<UnitResult>();

        let worker_count = self.jobs.min(total).max(1);
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            let handle = thread::spawn(move || loop {
                let unit = {
                    let mut q = match queue.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    q.pop_front()
                };
                let Some(unit) = unit else { break };

                let result = run_compile(&unit);
                if tx.send(result).is_err() {
                    break;
                }
            });
            handles.push(handle);
        }
        drop(tx);

        let mut failures = 0usize;
        for result in rx {
            for d in &result.diagnostics {
                eprint!("{}", d.render());
            }
            if !result.success {
                failures += 1;
                if result.diagnostics.is_empty() && !result.raw_stderr.trim().is_empty() {
                    // clang exited non-zero but its stderr didn't parse into
                    // any structured diagnostic — surface it raw rather than
                    // silently swallowing the only clue about the failure.
                    eprintln!("{}", result.raw_stderr.trim_end());
                }
            }
        }

        for handle in handles {
            let _ = handle.join();
        }

        if failures > 0 {
            return Err(CbldError::Analysis { failures });
        }

        if !self.quiet {
            println!(
                "\x1b[1;32m    Finished\x1b[0m static analysis: {} file{} checked",
                total,
                plural(total),
            );
        }
        Ok(())
    }

    /// Run all compile units across a fixed-size thread pool.
    fn compile_all(&self, units: Vec<CompileUnit>) -> Result<Vec<PathBuf>> {
        let total = units.len();
        let objects: Vec<PathBuf> = units.iter().map(|u| u.object.clone()).collect();

        // Shared work queue + results channel.
        let queue: Arc<Mutex<VecDeque<CompileUnit>>> = Arc::new(Mutex::new(VecDeque::from(units)));
        let (tx, rx) = mpsc::channel::<UnitResult>();

        let worker_count = self.jobs.min(total).max(1);
        let mut handles = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            let handle = thread::spawn(move || {
                loop {
                    // Pop one unit; release the lock before doing slow I/O.
                    let unit = {
                        let mut q = match queue.lock() {
                            Ok(g) => g,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        q.pop_front()
                    };
                    let Some(unit) = unit else { break };

                    let result = run_compile(&unit);
                    // If the receiver hung up, just stop.
                    if tx.send(result).is_err() {
                        break;
                    }
                }
            });
            handles.push(handle);
        }
        // Drop our own sender so the channel closes once workers finish.
        drop(tx);

        // Collect results as they arrive.
        let mut failures = 0usize;
        let mut completed = 0usize;
        let mut failed_diagnostics: Vec<CompileDiagnostic> = Vec::new();
        for result in rx {
            completed += 1;
            self.report_unit(&result, completed, total);
            if !result.success {
                failures += 1;
                collect_failure_diagnostics(&result, &mut failed_diagnostics);
            }
        }

        for handle in handles {
            // A panicked worker shouldn't abort the whole process silently.
            let _ = handle.join();
        }

        if failures > 0 {
            return Err(CbldError::Compilation {
                failures,
                diagnostics: failed_diagnostics,
            });
        }
        Ok(objects)
    }

    /// Print diagnostics for a finished unit. A no-op in `--json` mode — the
    /// caller renders diagnostics from the returned `CbldError::Compilation`
    /// (or the success payload) as a single structured object instead.
    fn report_unit(&self, result: &UnitResult, idx: usize, total: usize) {
        if self.json {
            return;
        }
        if result.success {
            if self.verbose {
                eprintln!(
                    "  \x1b[2m[{idx}/{total}]\x1b[0m \x1b[32mok\x1b[0m {}",
                    result.source.display()
                );
            }
            // Surface warnings even on success.
            for d in &result.diagnostics {
                if d.severity != Severity::Error {
                    eprint!("{}", d.render());
                }
            }
            return;
        }

        eprintln!(
            "\x1b[1;31merror\x1b[0m: failed to compile \x1b[1m{}\x1b[0m",
            result.source.display()
        );
        if result.diagnostics.is_empty() {
            // Fall back to raw stderr if we couldn't parse anything structured.
            eprintln!("{}", result.raw_stderr.trim_end());
        } else {
            for d in &result.diagnostics {
                eprint!("{}", d.render());
            }
        }
    }

    /// Execute the link/archive step.
    ///
    /// `candidates` is ordered most-preferred first. Linux has one (`ar`);
    /// macOS and Windows may list several (`llvm-ar`, then `ar` or `lib.exe`)
    /// since either could be the one actually installed. A
    /// candidate is skipped — not failed — only when the program itself can't
    /// be spawned; once a linker/archiver actually runs, its exit code is
    /// authoritative and reported as a real failure.
    fn run_link(&self, candidates: &[LinkCommand], kind: Crate) -> Result<()> {
        let mut last_spawn_err = None;

        for (i, link) in candidates.iter().enumerate() {
            if !self.quiet {
                let (verb, color) = match kind {
                    Crate::Executable => ("Linking", "\x1b[1;96m"),
                    Crate::Library => ("Archiving", "\x1b[1;93m"),
                    Crate::Shared => ("Linking", "\x1b[1;95m"),
                    Crate::Header => ("Header", "\x1b[1;92m"),
                };
                println!("{color}     {verb}\x1b[0m via \x1b[96m{}\x1b[0m", link.program);
            }
            if self.verbose {
                eprintln!(
                    "  \x1b[2m[engine]\x1b[0m {} {}",
                    link.program,
                    link.args.join(" ")
                );
            }

            let output = match driver_command(&link.program).args(&link.args).output() {
                Ok(out) => out,
                Err(source) => {
                    let is_last = i + 1 == candidates.len();
                    if !is_last {
                        if self.verbose {
                            eprintln!(
                                "  \x1b[2m[engine]\x1b[0m '{}' not found; trying next archiver",
                                link.program
                            );
                        }
                        last_spawn_err = Some(CbldError::CommandSpawn {
                            program: link.program.clone(),
                            source,
                        });
                        continue;
                    }
                    return Err(CbldError::CommandSpawn {
                        program: link.program.clone(),
                        source,
                    });
                }
            };

            if !output.status.success() {
                // Link errors also benefit from the diagnostics parser.
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let diags = parse_clang_diagnostics(&stderr);
                for d in &diags {
                    eprint!("{}", d.render());
                }
                if diags.is_empty() {
                    eprintln!("{}", stderr.trim_end());
                }
                return Err(CbldError::CommandFailed {
                    program: link.program.clone(),
                    code: output.status.code(),
                    stderr,
                });
            }
            return Ok(());
        }

        // Unreachable in practice — `link_command` never returns an empty
        // candidate list — but keeps the function total rather than panicking.
        Err(last_spawn_err
            .unwrap_or_else(|| CbldError::Config("no archiver/linker candidates available".into())))
    }
}

/// Fold one failed unit's diagnostics into the running list carried by
/// `CbldError::Compilation`. Prefers parsed `Error`-severity diagnostics;
/// falls back to a single synthetic entry built from raw stderr (or a
/// generic message) when clang's output couldn't be parsed at all.
fn collect_failure_diagnostics(result: &UnitResult, out: &mut Vec<CompileDiagnostic>) {
    let errors: Vec<&Diagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();

    if errors.is_empty() {
        let message = if result.raw_stderr.trim().is_empty() {
            "compilation failed with no diagnostic output".to_string()
        } else {
            result.raw_stderr.trim().to_string()
        };
        out.push(CompileDiagnostic {
            file: result.source.clone(),
            line: 0,
            column: 0,
            severity: "error",
            message,
        });
        return;
    }

    for d in errors {
        out.push(CompileDiagnostic {
            file: d.file.clone(),
            line: d.line,
            column: d.column,
            severity: "error",
            message: d.message.clone(),
        });
    }
}

/// Compile a single unit by invoking clang/clang++.
fn run_compile(unit: &CompileUnit) -> UnitResult {
    let driver = unit.language.driver();
    let output = driver_command(driver).args(&unit.args).output();

    match output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let diagnostics = parse_clang_diagnostics(&stderr);
            UnitResult {
                source: unit.source.clone(),
                success: out.status.success(),
                diagnostics,
                raw_stderr: stderr,
            }
        }
        Err(e) => UnitResult {
            source: unit.source.clone(),
            success: false,
            diagnostics: vec![Diagnostic {
                severity: Severity::Error,
                file: unit.source.clone(),
                line: 0,
                column: 0,
                message: format!("could not launch '{driver}': {e} (is clang installed?)"),
                code: None,
                snippet: None,
            }],
            raw_stderr: String::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// Clang diagnostics parsing
// ---------------------------------------------------------------------------

/// Severity of a parsed diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    fn parse(s: &str) -> Option<Severity> {
        match s {
            "error" | "fatal error" => Some(Severity::Error),
            "warning" => Some(Severity::Warning),
            "note" => Some(Severity::Note),
            _ => None,
        }
    }

    fn color(self) -> &'static str {
        match self {
            Severity::Error => "\x1b[1;31m",   // bold red
            Severity::Warning => "\x1b[1;33m", // bold yellow
            Severity::Note => "\x1b[1;36m",    // bold cyan
        }
    }

    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

/// A single structured diagnostic extracted from clang's stderr.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub file: PathBuf,
    pub line: usize,
    pub column: usize,
    pub message: String,
    /// e.g. `-Wunused-variable` if clang attributed the diagnostic to a flag.
    pub code: Option<String>,
    /// The source line clang echoed, if any.
    pub snippet: Option<String>,
}

impl Diagnostic {
    /// Render a clean, colorized, human-readable block for the terminal.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let color = self.severity.color();
        let reset = "\x1b[0m";

        // Header line: severity[code]: message
        out.push_str(color);
        out.push_str(self.severity.label());
        out.push_str(reset);
        if let Some(code) = &self.code {
            out.push_str(&format!("\x1b[2m[{code}]\x1b[0m"));
        }
        out.push_str(&format!(": {}\n", self.message));

        // Location line: --> file:line:col
        if self.line > 0 {
            out.push_str(&format!(
                "  \x1b[1;34m-->\x1b[0m {}:{}:{}\n",
                self.file.display(),
                self.line,
                self.column
            ));
        } else {
            out.push_str(&format!("  \x1b[1;34m-->\x1b[0m {}\n", self.file.display()));
        }

        // Optional source snippet.
        if let Some(snippet) = &self.snippet {
            out.push_str(&format!(
                "   \x1b[2m{:>4} |\x1b[0m {}\n",
                self.line, snippet
            ));
            if self.column > 0 {
                let pad = " ".repeat(self.column.saturating_sub(1));
                out.push_str(&format!(
                    "        \x1b[2m|\x1b[0m {}{}^{}\n",
                    pad, color, reset
                ));
            }
        }
        out.push('\n');
        out
    }
}

/// Parse clang/clang++ stderr into structured diagnostics.
///
/// Recognizes the canonical clang format:
///   `path/to/file.cpp:LINE:COL: severity: message [-Wsomething]`
/// followed optionally by a source snippet line and a caret line. Lines that
/// don't match a header are attached to the previous diagnostic as a snippet.
pub fn parse_clang_diagnostics(stderr: &str) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut lines = stderr.lines().peekable();

    while let Some(line) = lines.next() {
        if let Some(mut diag) = parse_header_line(line) {
            // Peek for a snippet line (next non-empty line that isn't another
            // header and isn't a bare caret).
            if let Some(next) = lines.peek() {
                let trimmed_next = next.trim_start();
                let is_header = parse_header_line(next).is_some();
                let is_caret = trimmed_next
                    .chars()
                    .all(|c| c == '^' || c == '~' || c == ' ')
                    && trimmed_next.contains('^');
                if !is_header && !is_caret && !next.trim().is_empty() {
                    diag.snippet = Some((*next).to_string());
                    lines.next();
                    // Consume a following caret line if present.
                    if let Some(after) = lines.peek() {
                        let t = after.trim_start();
                        let caret =
                            t.chars().all(|c| c == '^' || c == '~' || c == ' ') && t.contains('^');
                        if caret {
                            lines.next();
                        }
                    }
                }
            }
            diagnostics.push(diag);
        }
    }

    diagnostics
}

/// Try to parse a single clang header line into a `Diagnostic`.
fn parse_header_line(line: &str) -> Option<Diagnostic> {
    // Split off an optional trailing ` [-Wflag]` or ` [flag]` code.
    let (head, code) = match line.rfind(" [") {
        Some(idx) if line.ends_with(']') => {
            let code = &line[idx + 2..line.len() - 1];
            (&line[..idx], Some(code.to_string()))
        }
        _ => (line, None),
    };

    // Expect: file:line:col: severity: message
    // Match the severity marker first, then split location from the right so a
    // Windows drive letter (`C:\src\file.cpp:10:5: error: ...`) is not treated
    // as the file path.
    const MARKERS: [(&str, &str); 4] = [
        (": fatal error: ", "fatal error"),
        (": error: ", "error"),
        (": warning: ", "warning"),
        (": note: ", "note"),
    ];
    let (loc, sev_str, message) = MARKERS.iter().find_map(|(marker, sev)| {
        let idx = head.find(marker)?;
        Some((
            head[..idx].trim(),
            *sev,
            head[idx + marker.len()..].trim().to_string(),
        ))
    })?;

    let (path_and_line, col_str) = loc.rsplit_once(':')?;
    let (file, line_str) = path_and_line.rsplit_once(':')?;
    let line_no: usize = line_str.trim().parse().ok()?;
    let col_no: usize = col_str.trim().parse().ok()?;
    let severity = Severity::parse(sev_str)?;

    Some(Diagnostic {
        severity,
        file: PathBuf::from(file.trim()),
        line: line_no,
        column: col_no,
        message,
        code,
        snippet: None,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the artifact path for a package. Naming is platform-specific:
/// Windows wants `name.exe` / `name.lib`, Unix wants bare `name` /
/// `libname.a`.
fn artifact_path(
    profile_dir: &Path,
    kind: Crate,
    package_name: &str,
    output_name: Option<&str>,
) -> PathBuf {
    let name = output_name.unwrap_or(package_name);
    let filename = match kind {
        Crate::Executable => {
            if cfg!(target_os = "windows") {
                format!("{name}.exe")
            } else {
                name.to_string()
            }
        }
        Crate::Library => {
            if cfg!(target_os = "windows") {
                format!("{name}.lib")
            } else {
                format!("lib{name}.a")
            }
        }
        Crate::Shared => {
            if cfg!(target_os = "windows") {
                format!("{name}.dll")
            } else if cfg!(target_os = "macos") {
                format!("lib{name}.dylib")
            } else {
                format!("lib{name}.so")
            }
        }
        Crate::Header => name.to_string(),
    };
    profile_dir.join(filename)
}

/// Compute the object-file path for a source, mirroring its path under `src/`
/// to avoid collisions between same-named files in different directories.
fn object_path(obj_dir: &Path, layout: &Layout, source: &Path) -> PathBuf {
    let rel = source.strip_prefix(&layout.src).unwrap_or(source);
    let mut flat = rel.to_string_lossy().replace(['/', '\\'], "__");
    flat.push('.');
    flat.push_str(object_extension());
    obj_dir.join(flat)
}

/// `.obj` on Windows (MSVC/llvm-ar/lib.exe convention), `.o` everywhere else.
fn object_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        "obj"
    } else {
        "o"
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn object_is_fresh(source: &Path, object: &Path, depfile: &Path) -> bool {
    let Ok(obj_meta) = fs::metadata(object) else {
        return false;
    };
    let Ok(obj_mtime) = obj_meta.modified() else {
        return false;
    };
    let Ok(src_meta) = fs::metadata(source) else {
        return false;
    };
    let Ok(src_mtime) = src_meta.modified() else {
        return false;
    };
    if src_mtime > obj_mtime {
        return false;
    }
    // If depfile exists, check all header deps.
    if let Ok(text) = fs::read_to_string(depfile) {
        for dep in parse_depfile(&text) {
            // dep may be absolute or relative; try as is, then relative to depfile dir.
            let dep_path = PathBuf::from(&dep);
            let candidate = if dep_path.is_absolute() {
                dep_path
            } else {
                depfile.parent().unwrap_or(Path::new(".")).join(&dep_path)
            };
            // Also try source dir relative? depfile stores relative to build dir but we store absolute?
            // Most deps are absolute or relative to package root via -I. Try both.
            let check_paths = [
                candidate.clone(),
                // Also try as absolute via current dir
                PathBuf::from(&dep),
            ];
            for p in &check_paths {
                if let Ok(meta) = fs::metadata(p) {
                    if let Ok(mtime) = meta.modified() {
                        if mtime > obj_mtime {
                            return false;
                        }
                    }
                    break;
                }
            }
        }
    }
    true
}

fn parse_depfile(text: &str) -> Vec<String> {
    // depfile is Makefile: "obj.o: src.c header.h \"
    // Handle line continuations and split.
    let mut cleaned = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&'\n') = chars.peek() {
                chars.next();
                cleaned.push(' ');
                continue;
            }
        }
        if c == '\n' {
            cleaned.push(' ');
        } else {
            cleaned.push(c);
        }
    }
    // Find colon, deps are after it.
    let Some(colon) = cleaned.find(':') else {
        return Vec::new();
    };
    let after = &cleaned[colon + 1..];
    after
        .split_whitespace()
        .filter(|s| *s != "\\")
        .map(|s| s.to_string())
        .collect()
}

/// Determine a sensible default parallelism when `-j` is not provided.
pub fn default_jobs() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Convenience: load a package's `[package]` table or error helpfully.
pub fn require_package(manifest: &Manifest, root: &Path) -> Result<Package> {
    manifest
        .package
        .clone()
        .ok_or_else(|| CbldError::ManifestParse {
            path: root.join("cbld.toml"),
            message: "missing [package] table (name/version required to build)".to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Compiler;
    use crate::manifest::{CProfile, CppProfile};
    use std::process::Command;
    use std::sync::Mutex;

    /// Env vars are process-global; serialize the one test below that
    /// mutates `CBLD_HOME` so it can't race with itself across reruns.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn make_library_package(dir: &Path, name: &str) -> (Layout, Package) {
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("lib.c"),
            "int cbld_add(int a, int b) { return a + b; }\n",
        )
        .unwrap();
        let layout = Layout::discover(dir, &ScanConfig::strict("src")).unwrap();
        let package = Package {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            description: None,
            authors: Vec::new(),
            toolchain: None,
            target: None,
            source_dir: "src".to_string(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            libs: Vec::new(),
            pkg_config: Vec::new(),
            ignore_warnings: false,
            kind: None,
            include: Vec::new(),
            exclude: Vec::new(),
        };
        (layout, package)
    }

    /// A custom `source_dir` (legacy support) must be honored end-to-end: the
    /// entry point is looked up under that directory, not the hardcoded
    /// `src/`. A `src/`-less tree that would fail under the default must
    /// succeed once pointed at the right directory.
    #[test]
    fn discover_honors_custom_source_dir_and_rejects_missing_one() {
        let tmp = std::env::temp_dir().join(format!("cbld-srcdir-{}", std::process::id()));
        let legacy = tmp.join("legacy");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("main.c"), "int main(void){return 0;}\n").unwrap();

        // Default "src" doesn't exist -> layout violation.
        assert!(Layout::discover(&tmp, &ScanConfig::strict("src")).is_err());
        // Pointed at the real directory -> discovered as an executable.
        let layout = Layout::discover(&tmp, &ScanConfig::strict("legacy")).unwrap();
        assert_eq!(layout.crate_kind, Crate::Executable);
        assert_eq!(layout.entry_language, Language::C);

        fs::remove_dir_all(&tmp).ok();
    }

    /// Legacy entry points with extended extensions must be discovered too: a
    /// `main.C` is an executable whose language is C++ (capital `.C`), not just
    /// the canonical `main.cpp`/`main.c`.
    #[test]
    fn discover_accepts_legacy_entry_extensions() {
        let tmp = std::env::temp_dir().join(format!("cbld-entryext-{}", std::process::id()));
        let src = tmp.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("main.C"), "int main(){return 0;}\n").unwrap();

        let layout = Layout::discover(&tmp, &ScanConfig::strict("src")).unwrap();
        assert_eq!(layout.crate_kind, Crate::Executable);
        assert_eq!(layout.entry_language, Language::Cpp);
        assert_eq!(layout.entry.unwrap().file_name().unwrap(), "main.C");

        fs::remove_dir_all(&tmp).ok();
    }

    /// 0.7 — `[package] kind` removes the canonical-entry requirement: a
    /// directory of arbitrarily-named sources (no `main.*`/`lib.*`) builds as
    /// the declared kind, with the language inferred from the sources.
    #[test]
    fn kind_lib_builds_arbitrarily_named_sources_without_a_canonical_entry() {
        let tmp = std::env::temp_dir().join(format!("cbld-kind-{}", std::process::id()));
        let src = tmp.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("cJSON.c"), "int cjson_x(void){return 1;}\n").unwrap();
        fs::write(src.join("util.c"), "int util_y(void){return 2;}\n").unwrap();

        // Without kind (and no main/lib entry) -> helpful error.
        assert!(Layout::discover(&tmp, &ScanConfig::strict("src")).is_err());

        // With kind = lib -> accepted; language inferred as C.
        let cfg = ScanConfig {
            source_dir: "src".to_string(),
            kind: Some(Crate::Library),
            ..ScanConfig::default()
        };
        let layout = Layout::discover(&tmp, &cfg).unwrap();
        assert_eq!(layout.crate_kind, Crate::Library);
        assert_eq!(layout.entry_language, Language::C);
        assert!(layout.entry.is_none());
        assert_eq!(layout.collect_sources().unwrap().len(), 2);

        fs::remove_dir_all(&tmp).ok();
    }

    /// Header-only packages do not need a source directory: there is nothing
    /// to compile. `as_library` must not promote them into an archive.
    #[test]
    fn kind_header_does_not_require_a_source_directory() {
        let tmp = std::env::temp_dir().join(format!("cbld-header-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("include").join("nlohmann")).unwrap();
        fs::write(
            tmp.join("include").join("nlohmann").join("json.hpp"),
            "#pragma once\n",
        )
        .unwrap();

        let cfg = ScanConfig {
            source_dir: "src".to_string(),
            kind: Some(Crate::Header),
            ..ScanConfig::default()
        };
        let layout = Layout::discover(&tmp, &cfg).unwrap();
        assert_eq!(layout.crate_kind, Crate::Header);
        assert_eq!(layout.as_library().crate_kind, Crate::Header);
        assert!(Crate::parse("header-only").unwrap() == Crate::Header);
        let formatted = layout.collect_format_files().unwrap();
        assert_eq!(formatted.len(), 1);
        assert!(formatted[0].ends_with("json.hpp"));

        fs::remove_dir_all(&tmp).ok();
    }

    /// 0.7 — `exclude` globs prune a vendored repo's tests/fuzzers so they
    /// aren't swept into the library; `include` further narrows the scan.
    #[test]
    fn exclude_and_include_globs_narrow_the_scan() {
        let tmp = std::env::temp_dir().join(format!("cbld-glob-{}", std::process::id()));
        let root = tmp.join("proj");
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::create_dir_all(root.join("fuzzing")).unwrap();
        fs::write(root.join("cJSON.c"), "int a(void){return 0;}\n").unwrap();
        fs::write(root.join("cJSON_Utils.c"), "int b(void){return 0;}\n").unwrap();
        fs::write(root.join("tests/test.c"), "int main(void){return 0;}\n").unwrap();
        fs::write(root.join("fuzzing/fuzz.c"), "int c(void){return 0;}\n").unwrap();

        // source_dir = "." with kind = lib, excluding tests + fuzzers.
        let cfg = ScanConfig {
            source_dir: ".".to_string(),
            kind: Some(Crate::Library),
            include: Vec::new(),
            exclude: vec!["tests/**".to_string(), "fuzzing/**".to_string()],
        };
        let layout = Layout::discover(&root, &cfg).unwrap();
        let sources = layout.collect_sources().unwrap();
        assert_eq!(
            sources.len(),
            2,
            "only the two library sources survive the exclude"
        );
        assert!(sources.iter().all(|p| {
            let s = p.to_string_lossy();
            !s.contains("tests") && !s.contains("fuzzing")
        }));

        fs::remove_dir_all(&tmp).ok();
    }

    /// End-to-end: a fresh library build must populate
    /// `~/.cbld/cache/prebuilt/{hash}`, and an identical second build (even
    /// after the local `target/` is wiped) must be served from that cache
    /// instead of invoking the compiler again.
    #[test]
    fn build_package_populates_and_then_hits_the_global_cache() {
        if Command::new("clang").arg("--version").output().is_err() {
            eprintln!("skipping: clang not available in this environment");
            return;
        }
        let _guard = ENV_LOCK.lock().unwrap();

        let pid = std::process::id();
        let project = std::env::temp_dir().join(format!("cbld-engine-cache-test-{pid}"));
        let home = std::env::temp_dir().join(format!("cbld-engine-cache-home-{pid}"));
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&project).unwrap();

        let prev_home = std::env::var("CBLD_HOME").ok();
        std::env::set_var("CBLD_HOME", &home);

        let (layout, package) = make_library_package(&project, "cachelib");
        let compiler = Compiler::new(
            CProfile::default(),
            CppProfile::default(),
            &project,
            Vec::new(),
            &[],
            false,
            false,
            None,
            Vec::new(),
            false,
        );
        let engine = Engine::new(1, false, true, false, false);
        let target_dir = project.join("target");
        let dest = BuildDest {
            target_dir: &target_dir,
            output_name: None,
            release: false,
        };

        let first = engine
            .build_package(&layout, &package, &compiler, dest, &[])
            .unwrap();
        assert!(
            !first.cache_hit,
            "first build should compile, not hit the cache"
        );

        // Wipe the local target dir so the second build can only succeed by
        // copying from the *global* cache, not by reusing a local leftover.
        fs::remove_dir_all(&target_dir).unwrap();

        let second = engine
            .build_package(
                &layout,
                &package,
                &compiler,
                BuildDest {
                    target_dir: &target_dir,
                    output_name: None,
                    release: false,
                },
                &[],
            )
            .unwrap();
        assert!(
            second.cache_hit,
            "second build with identical inputs should hit the global cache"
        );
        assert!(second.path.is_file());

        match prev_home {
            Some(v) => std::env::set_var("CBLD_HOME", v),
            None => std::env::remove_var("CBLD_HOME"),
        }
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&home);
    }

    /// Dependency archives must actually reach the executable link line.
    /// Header-only deps are a no-op here; compiled libs (cJSON, fmt) are not.
    #[test]
    fn executable_links_dependency_archives() {
        if Command::new("clang").arg("--version").output().is_err() {
            eprintln!("skipping: clang not available in this environment");
            return;
        }
        let _guard = ENV_LOCK.lock().unwrap();

        let pid = std::process::id();
        let root = std::env::temp_dir().join(format!("cbld-engine-link-{pid}"));
        let home = std::env::temp_dir().join(format!("cbld-engine-link-home-{pid}"));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&home);
        let prev_home = std::env::var("CBLD_HOME").ok();
        std::env::set_var("CBLD_HOME", &home);

        let lib_root = root.join("lib");
        let exe_root = root.join("app");
        fs::create_dir_all(&lib_root).unwrap();
        fs::create_dir_all(exe_root.join("src")).unwrap();

        let (lib_layout, lib_pkg) = make_library_package(&lib_root, "add");
        let lib_compiler = Compiler::new(
            CProfile::default(),
            CppProfile::default(),
            &lib_root,
            Vec::new(),
            &[],
            false,
            false,
            None,
            Vec::new(),
            false,
        );
        let engine = Engine::new(1, false, true, false, false);
        let lib_target = lib_root.join("target");
        let lib_built = engine
            .build_package(
                &lib_layout,
                &lib_pkg,
                &lib_compiler,
                BuildDest {
                    target_dir: &lib_target,
                    output_name: None,
                    release: false,
                },
                &[],
            )
            .unwrap();
        assert!(lib_built.path.is_file());

        fs::write(
            exe_root.join("src").join("main.c"),
            "int cbld_add(int, int);\nint main(void) { return cbld_add(2, 3) == 5 ? 0 : 1; }\n",
        )
        .unwrap();
        let exe_layout = Layout::discover(&exe_root, &ScanConfig::strict("src")).unwrap();
        let exe_pkg = Package {
            name: "app".to_string(),
            version: "0.1.0".to_string(),
            description: None,
            authors: Vec::new(),
            toolchain: None,
            target: None,
            source_dir: "src".to_string(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            libs: Vec::new(),
            pkg_config: Vec::new(),
            ignore_warnings: false,
            kind: None,
            include: Vec::new(),
            exclude: Vec::new(),
        };
        let exe_compiler = Compiler::new(
            CProfile::default(),
            CppProfile::default(),
            &exe_root,
            Vec::new(),
            &[],
            false,
            false,
            None,
            Vec::new(),
            false,
        );
        let exe_target = exe_root.join("target");
        let exe_built = engine
            .build_package(
                &exe_layout,
                &exe_pkg,
                &exe_compiler,
                BuildDest {
                    target_dir: &exe_target,
                    output_name: None,
                    release: false,
                },
                std::slice::from_ref(&lib_built.path),
            )
            .unwrap();
        let status = Command::new(&exe_built.path).status().unwrap();
        assert!(status.success(), "linked executable should run");

        match prev_home {
            Some(v) => std::env::set_var("CBLD_HOME", v),
            None => std::env::remove_var("CBLD_HOME"),
        }
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn collect_failure_diagnostics_falls_back_to_raw_stderr_when_unparsed() {
        let result = UnitResult {
            source: PathBuf::from("src/main.c"),
            success: false,
            diagnostics: Vec::new(),
            raw_stderr: "some opaque linker-style failure".to_string(),
        };
        let mut out = Vec::new();
        collect_failure_diagnostics(&result, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, "error");
        assert!(out[0].message.contains("opaque linker-style failure"));
    }

    #[test]
    fn collect_failure_diagnostics_prefers_parsed_error_severity() {
        let result = UnitResult {
            source: PathBuf::from("src/main.c"),
            success: false,
            diagnostics: vec![
                Diagnostic {
                    severity: Severity::Warning,
                    file: PathBuf::from("src/main.c"),
                    line: 1,
                    column: 1,
                    message: "unused variable".into(),
                    code: None,
                    snippet: None,
                },
                Diagnostic {
                    severity: Severity::Error,
                    file: PathBuf::from("src/main.c"),
                    line: 2,
                    column: 3,
                    message: "undeclared identifier".into(),
                    code: None,
                    snippet: None,
                },
            ],
            raw_stderr: String::new(),
        };
        let mut out = Vec::new();
        collect_failure_diagnostics(&result, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].line, 2);
        assert_eq!(out[0].message, "undeclared identifier");
    }

    #[test]
    fn artifact_path_matches_platform_convention() {
        let dir = PathBuf::from("/tmp/profile");
        let exe = artifact_path(&dir, Crate::Executable, "app", None);
        let lib = artifact_path(&dir, Crate::Library, "mylib", None);
        if cfg!(target_os = "windows") {
            assert_eq!(exe.file_name().unwrap(), "app.exe");
            assert_eq!(lib.file_name().unwrap(), "mylib.lib");
        } else {
            assert_eq!(exe.file_name().unwrap(), "app");
            assert_eq!(lib.file_name().unwrap(), "libmylib.a");
        }
        let overridden = artifact_path(&dir, Crate::Library, "mylib", Some("custom"));
        assert!(overridden
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("custom"));
    }

    #[test]
    fn parse_header_line_unix_and_windows_paths() {
        let unix = parse_clang_diagnostics(
            "src/main.c:10:5: error: undeclared identifier [-Werror]\nint x;\n    ^\n",
        );
        assert_eq!(unix.len(), 1);
        assert_eq!(unix[0].file, PathBuf::from("src/main.c"));
        assert_eq!(unix[0].line, 10);
        assert_eq!(unix[0].column, 5);
        assert_eq!(unix[0].message, "undeclared identifier");
        assert_eq!(unix[0].code.as_deref(), Some("-Werror"));

        let win = parse_clang_diagnostics(
            r"C:\Users\dev\src\main.cpp:12:3: warning: unused variable [-Wunused-variable]",
        );
        assert_eq!(win.len(), 1);
        assert_eq!(win[0].file, PathBuf::from(r"C:\Users\dev\src\main.cpp"));
        assert_eq!(win[0].line, 12);
        assert_eq!(win[0].column, 3);
        assert_eq!(win[0].severity, Severity::Warning);
    }
}
