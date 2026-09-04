//! Compiler subsystem: translate manifest profiles into precise `clang` /
//! `clang++` argument vectors.
//!
//! Strict C / C++ separation is enforced at the type level: a translation unit
//! is *either* `Language::C` *or* `Language::Cpp`, and the function that builds
//! the argument vector takes the matching profile. There is no code path that
//! lets C flags leak into a C++ invocation or vice versa.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{CbldError, Result};
use crate::manifest::{CProfile, CppProfile};

/// The two — and only two — languages cbld compiles. They are kept rigidly
/// distinct to avoid ABI and standard-mismatch bugs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    C,
    Cpp,
}

impl Language {
    /// Driver executable candidates for this language (Clang primary, GCC, TCC, Zig CC fallback).
    pub fn driver_candidates(self) -> &'static [&'static str] {
        match self {
            Language::C => &["clang", "gcc", "tcc", "zig cc", "cc"],
            Language::Cpp => &["clang++", "g++", "zig c++", "c++"],
        }
    }

    /// The compiler driver to invoke for this language (auto-detects clang -> gcc -> cc).
    pub fn driver(self) -> &'static str {
        for &candidate in self.driver_candidates() {
            if driver_command(candidate).arg("--version").output().is_ok() {
                return candidate;
            }
        }
        match self {
            Language::C => "clang",
            Language::Cpp => "clang++",
        }
    }

    /// Recognize a source file's language purely from its extension.
    ///
    /// Returns `None` for headers and unknown extensions — those are never
    /// compiled as translation units.
    pub fn from_extension(path: &Path) -> Option<Language> {
        let raw = path.extension()?.to_str()?;
        // A capital `.C` is C++ by long-standing Unix convention (GCC/Clang
        // both treat it that way). This must be checked *before* lowercasing,
        // or `.C` would collapse into the lowercase `"c"` arm and be
        // mis-compiled as C.
        if raw == "C" {
            return Some(Language::Cpp);
        }
        let ext = raw.to_ascii_lowercase();
        match ext.as_str() {
            "c" => Some(Language::C),
            "cc" | "cpp" | "cxx" | "c++" | "cp" | "cppm" | "ccm" | "cxxm" | "c++m" | "ixx"
            | "mxx" => Some(Language::Cpp),
            _ => None,
        }
    }

    /// True for standard C++ module interface/implementation extensions.
    pub fn is_module_source(path: &Path) -> bool {
        let Some(raw) = path.extension().and_then(|extension| extension.to_str()) else {
            return false;
        };
        matches!(
            raw.to_ascii_lowercase().as_str(),
            "cppm" | "ccm" | "cxxm" | "c++m" | "ixx" | "mxx"
        )
    }

    /// True for C/C++ header extensions. Headers are never translation units
    /// (`from_extension` returns `None`); `cbld fmt` / `cbld lint` still
    /// visit them.
    pub fn is_header(path: &Path) -> bool {
        let Some(raw) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        matches!(
            raw.to_ascii_lowercase().as_str(),
            "h" | "hh" | "hpp" | "hxx" | "h++" | "inl"
        )
    }
}

/// Build a `Command` for a driver name that may include arguments (`zig cc`).
pub fn driver_command(driver: &str) -> Command {
    let mut parts = driver.split_whitespace();
    let prog = parts.next().unwrap_or(driver);
    let mut cmd = Command::new(prog);
    for arg in parts {
        cmd.arg(arg);
    }
    cmd
}

/// Optimization level, parsed from the manifest string into a closed set so we
/// never hand clang an unvalidated `-O<garbage>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    O0,
    O1,
    O2,
    O3,
    Osize,  // -Os
    Oztiny, // -Oz
    Odebug, // -Og
    Ofast,  // -Ofast
}

impl OptLevel {
    pub fn parse(raw: &str) -> Result<OptLevel> {
        Ok(match raw.trim() {
            "0" => OptLevel::O0,
            "1" => OptLevel::O1,
            "2" => OptLevel::O2,
            "3" => OptLevel::O3,
            "s" | "size" => OptLevel::Osize,
            "z" | "tiny" => OptLevel::Oztiny,
            "g" | "debug" => OptLevel::Odebug,
            "fast" => OptLevel::Ofast,
            other => {
                return Err(CbldError::Config(format!(
                    "unknown optimization level '{other}' (expected 0,1,2,3,s,z,g,fast)"
                )));
            }
        })
    }

    pub fn flag(self) -> &'static str {
        match self {
            OptLevel::O0 => "-O0",
            OptLevel::O1 => "-O1",
            OptLevel::O2 => "-O2",
            OptLevel::O3 => "-O3",
            OptLevel::Osize => "-Os",
            OptLevel::Oztiny => "-Oz",
            OptLevel::Odebug => "-Og",
            OptLevel::Ofast => "-Ofast",
        }
    }
}

/// A Clang sanitizer, parsed from the manifest's `sanitizers` array into a
/// closed set so an unrecognized name fails fast instead of silently being
/// dropped or mis-forwarded to clang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sanitizer {
    Address,
    Thread,
    Undefined,
    Leak,
}

impl Sanitizer {
    pub fn parse(raw: &str) -> Result<Sanitizer> {
        Ok(match raw.trim() {
            "address" => Sanitizer::Address,
            "thread" => Sanitizer::Thread,
            "undefined" => Sanitizer::Undefined,
            "leak" => Sanitizer::Leak,
            other => {
                return Err(CbldError::Config(format!(
                    "unknown sanitizer '{other}' (expected address, thread, undefined, leak)"
                )));
            }
        })
    }

    /// The exact `-fsanitize=<name>` clang flag, shared verbatim between the
    /// compile and link commands so both phases agree on instrumentation.
    pub fn flag(self) -> &'static str {
        match self {
            Sanitizer::Address => "-fsanitize=address",
            Sanitizer::Thread => "-fsanitize=thread",
            Sanitizer::Undefined => "-fsanitize=undefined",
            Sanitizer::Leak => "-fsanitize=leak",
        }
    }
}

/// Parse every entry of a manifest's `sanitizers` array, failing on the first
/// unrecognized name.
fn parse_sanitizers(raw: &[String]) -> Result<Vec<Sanitizer>> {
    raw.iter().map(|s| Sanitizer::parse(s)).collect()
}

/// The pre-build safety matrix: catches sanitizer/LTO combinations that clang
/// accepts syntactically but that are unsafe or unsupported at runtime, so
/// cbld aborts before spending any time compiling.
///
/// `label` is the profile name ("C" or "C++") for a precise error message.
fn validate_sanitizer_matrix(sanitizers: &[Sanitizer], lto: bool, label: &str) -> Result<()> {
    let has = |s: Sanitizer| sanitizers.contains(&s);

    if lto && (has(Sanitizer::Address) || has(Sanitizer::Leak)) {
        return Err(CbldError::Config(format!(
            "[profile.{}] enables LTO together with the address/leak sanitizer, which are \
             mutually exclusive: link-time optimization can reorder and inline across the \
             instrumentation boundary, producing unreliable ASan/LSan results and \
             substantially slower links. Disable `lto` or remove \"address\"/\"leak\" from \
             `sanitizers`.",
            label.to_ascii_lowercase()
        )));
    }

    if has(Sanitizer::Thread) && (has(Sanitizer::Address) || has(Sanitizer::Leak)) {
        return Err(CbldError::Config(format!(
            "[profile.{}] combines the thread sanitizer with the address/leak sanitizer: \
             their runtime libraries install conflicting interceptors and cannot be linked \
             into the same binary. Pick one sanitizer family at a time.",
            label.to_ascii_lowercase()
        )));
    }

    Ok(())
}

/// Map a warning keyword from the manifest to a clang `-W` flag.
///
/// Unknown keywords are passed through as `-W<keyword>` so users can name any
/// clang warning group without cbld needing an exhaustive table.
fn warning_flag(keyword: &str) -> String {
    match keyword.trim() {
        "all" => "-Wall".to_string(),
        "extra" => "-Wextra".to_string(),
        "error" => "-Werror".to_string(),
        "pedantic" => "-Wpedantic".to_string(),
        "everything" => "-Weverything".to_string(),
        other => format!("-W{other}"),
    }
}

/// A fully-specified compile job for one translation unit.
#[derive(Debug, Clone)]
pub struct CompileUnit {
    pub language: Language,
    pub source: PathBuf,
    pub object: PathBuf,
    /// The complete argument vector (excluding the driver program itself).
    pub args: Vec<String>,
}

/// Holds the resolved, validated compile settings for a single package and
/// produces per-unit argument vectors. Constructed once per build.
pub struct Compiler {
    c_profile: CProfile,
    cpp_profile: CppProfile,
    /// The package's own `include/` directory, always searched first so its
    /// public headers resolve regardless of what the caller passed in
    /// `include_dirs`.
    own_include_dir: PathBuf,
    /// `-I` include directories shared by both languages (dependency headers).
    include_dirs: Vec<PathBuf>,
    /// Generated configuration headers that must shadow an upstream tree's
    /// static fallback headers without modifying that checkout.
    priority_include_dirs: Vec<PathBuf>,
    /// `-D` defines injected for active features, e.g. `CBLD_FEATURE_SSL`.
    feature_defines: Vec<String>,
    /// Project-wide `-D` defines from `[package] defines`, applied to *both*
    /// languages on top of each profile's own `defines` (legacy support).
    package_defines: Vec<String>,
    /// When true, inject `-w` to silence every compiler warning (`[package]
    /// ignore_warnings` or `cbld build --ignore-warnings`). A blunt escape
    /// hatch for noisy legacy code.
    ignore_warnings: bool,
    /// When true, append `-g` and force a debug-friendly opt floor.
    debug: bool,
    /// Release builds set NDEBUG and trust the profile's optimization level.
    release: bool,
    /// When true, inject `-ftime-trace` so every translation unit emits a
    /// sibling `.json` profiling file next to its object (`cbld build
    /// --trace`); see `trace.rs` for how those files get aggregated.
    trace: bool,
    /// Cross-compilation target triple (`cbld build --target` or the
    /// `[package] target` manifest field). When set, injected as
    /// `--target=<triple>` into every compile *and* the final link step, so
    /// object files and the linked artifact always agree on target.
    target: Option<String>,
    /// Target sysroot selected by the matching manifest preset.
    sysroot: Option<PathBuf>,
}

impl Compiler {
    /// `package_root` is the root of the package being compiled (the
    /// directory containing its `cbld.toml`); its `include/` subdirectory is
    /// unconditionally added to the header search path, independent of
    /// `include_dirs` (which carries *other* packages' public headers, e.g.
    /// dependencies).
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        c_profile: CProfile,
        cpp_profile: CppProfile,
        package_root: &Path,
        include_dirs: Vec<PathBuf>,
        active_features: &[String],
        release: bool,
        trace: bool,
        target: Option<String>,
        package_defines: Vec<String>,
        ignore_warnings: bool,
    ) -> Compiler {
        Self::new_with_sysroot(
            c_profile,
            cpp_profile,
            package_root,
            include_dirs,
            active_features,
            release,
            trace,
            target,
            None,
            package_defines,
            ignore_warnings,
        )
    }

    /// Construct compiler settings with an optional cross-compilation sysroot.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_sysroot(
        c_profile: CProfile,
        cpp_profile: CppProfile,
        package_root: &Path,
        include_dirs: Vec<PathBuf>,
        active_features: &[String],
        release: bool,
        trace: bool,
        target: Option<String>,
        sysroot: Option<PathBuf>,
        package_defines: Vec<String>,
        ignore_warnings: bool,
    ) -> Compiler {
        let feature_defines = active_features
            .iter()
            .map(|f| format!("CBLD_FEATURE_{}", f.to_ascii_uppercase().replace('-', "_")))
            .collect();
        let include_dir = package_root.join("include");
        Compiler {
            c_profile,
            cpp_profile,
            // Absolute so the flag is correct regardless of clang's cwd at
            // invocation time; falls back to the joined (possibly relative)
            // path if the directory doesn't exist yet to canonicalize.
            own_include_dir: include_dir.canonicalize().unwrap_or(include_dir),
            include_dirs,
            priority_include_dirs: Vec::new(),
            feature_defines,
            package_defines,
            ignore_warnings,
            debug: !release,
            release,
            trace,
            target,
            sysroot,
        }
    }

    /// Add generated header roots ahead of the package's own `include/` tree.
    pub fn with_priority_include_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.priority_include_dirs = dirs;
        self
    }

    /// Generated include roots whose contents must participate in a library
    /// cache key, just like source-tree public headers.
    pub fn priority_include_dirs(&self) -> &[PathBuf] {
        &self.priority_include_dirs
    }

    /// Validate the profiles up front so a bad optimization level fails before
    /// any compilation begins, rather than mid-build.
    pub fn validate(&self) -> Result<()> {
        OptLevel::parse(&self.c_profile.optimization)?;
        OptLevel::parse(&self.cpp_profile.optimization)?;

        let c_sanitizers = parse_sanitizers(&self.c_profile.sanitizers)?;
        validate_sanitizer_matrix(&c_sanitizers, self.c_profile.lto, "C")?;
        self.warn_if_sanitizers_strip_symbols(&c_sanitizers);

        let cpp_sanitizers = parse_sanitizers(&self.cpp_profile.sanitizers)?;
        validate_sanitizer_matrix(&cpp_sanitizers, self.cpp_profile.lto, "C++")?;
        self.warn_if_sanitizers_strip_symbols(&cpp_sanitizers);

        Ok(())
    }

    /// Sanitizer stack traces are unreadable raw addresses without `-g`.
    /// `push_common` always forces `-g` in when sanitizers are active
    /// (correctness), but a release/optimized profile choosing to omit debug
    /// symbols is surprising enough that it deserves one up-front warning
    /// rather than a silently-overridden profile setting.
    fn warn_if_sanitizers_strip_symbols(&self, sanitizers: &[Sanitizer]) {
        if !sanitizers.is_empty() && self.release {
            eprintln!(
                "\x1b[1;33mwarning\x1b[0m: active sanitizers force `-g` (debug symbols) into \
                 this release/optimized build so stack traces stay readable"
            );
        }
    }

    /// Resolve the optimization level actually handed to clang.
    ///
    /// `--release` always wins with `-O3`: the manifest's `optimization` field
    /// describes the dev/debug profile, not a release override. This is the
    /// one place `--release` maps onto a concrete `-O` flag.
    fn effective_opt(&self, profile_opt: &str) -> Result<OptLevel> {
        if self.release {
            return Ok(OptLevel::O3);
        }
        OptLevel::parse(profile_opt)
    }

    /// Build the compile command for a single source file. The `object` path is
    /// where the `.o` should be written.
    pub fn compile_unit(&self, source: &Path, object: &Path) -> Result<CompileUnit> {
        let language = Language::from_extension(source).ok_or_else(|| {
            CbldError::Config(format!(
                "cannot determine language for '{}' (unsupported extension)",
                source.display()
            ))
        })?;

        let args = match language {
            Language::C => self.c_args(source, object)?,
            Language::Cpp => self.cpp_args(source, object)?,
        };

        Ok(CompileUnit {
            language,
            source: source.to_path_buf(),
            object: object.to_path_buf(),
            args,
        })
    }

    /// Build the `cbld check` static-analysis invocation for a single source
    /// file: `--analyze` in place of `-c -o <object>`, so clang parses and
    /// runs its analyzer matrix without emitting an object file. Reuses
    /// `CompileUnit`'s shape purely for its `language`/`source`/`args`
    /// fields and `run_compile`'s existing spawn-and-parse path in
    /// `engine.rs`; `object` is an unused placeholder since analysis never
    /// produces one.
    pub fn analyze_unit(&self, source: &Path) -> Result<CompileUnit> {
        let language = Language::from_extension(source).ok_or_else(|| {
            CbldError::Config(format!(
                "cannot determine language for '{}' (unsupported extension)",
                source.display()
            ))
        })?;

        let args = match language {
            Language::C => self.c_analyze_args(source),
            Language::Cpp => self.cpp_analyze_args(source),
        };

        Ok(CompileUnit {
            language,
            source: source.to_path_buf(),
            object: PathBuf::new(),
            args,
        })
    }

    /// Argument vector for a C translation unit. Only ever reads `c_profile`.
    fn c_args(&self, source: &Path, object: &Path) -> Result<Vec<String>> {
        let mut args = self.c_flags()?;
        args.push("-o".to_string());
        args.push(object.to_string_lossy().to_string());
        args.push(source.to_string_lossy().to_string());
        Ok(args)
    }

    /// Argument vector for a C++ translation unit. Only ever reads `cpp_profile`.
    fn cpp_args(&self, source: &Path, object: &Path) -> Result<Vec<String>> {
        let mut args = self.cpp_flags()?;
        args.push("-o".to_string());
        args.push(object.to_string_lossy().to_string());
        args.push(source.to_string_lossy().to_string());
        Ok(args)
    }

    /// The C flag set with no source/object paths baked in. Shared by
    /// `c_args` and [`Compiler::cache_fingerprint`].
    fn c_flags(&self) -> Result<Vec<String>> {
        let p = &self.c_profile;
        let opt = self.effective_opt(&p.optimization)?;
        let sanitizers = parse_sanitizers(&p.sanitizers)?;
        let mut args = Vec::new();

        args.push("-c".to_string());
        args.push(format!("-std={}", p.standard));
        args.push(opt.flag().to_string());
        for w in &p.warnings {
            args.push(warning_flag(w));
        }
        self.push_common(&mut args, &p.defines, !sanitizers.is_empty());
        if p.lto {
            args.push("-flto".to_string());
        }
        for s in &sanitizers {
            args.push(s.flag().to_string());
        }
        for extra in &p.extra_flags {
            args.push(extra.clone());
        }
        Ok(args)
    }

    /// The C++ flag set with no source/object paths baked in. Shared by
    /// `cpp_args` and [`Compiler::cache_fingerprint`].
    fn cpp_flags(&self) -> Result<Vec<String>> {
        let p = &self.cpp_profile;
        let opt = self.effective_opt(&p.optimization)?;
        let sanitizers = parse_sanitizers(&p.sanitizers)?;
        let mut args = Vec::new();

        args.push("-c".to_string());
        args.push(format!("-std={}", p.standard));
        args.push(opt.flag().to_string());

        // RTTI / exceptions are C++-only toggles; encode them precisely.
        if p.rtti {
            args.push("-frtti".to_string());
        } else {
            args.push("-fno-rtti".to_string());
        }
        if p.exceptions {
            args.push("-fexceptions".to_string());
        } else {
            args.push("-fno-exceptions".to_string());
        }

        for w in &p.warnings {
            args.push(warning_flag(w));
        }
        self.push_common(&mut args, &p.defines, !sanitizers.is_empty());
        if p.lto {
            args.push("-flto".to_string());
        }
        for s in &sanitizers {
            args.push(s.flag().to_string());
        }
        for extra in &p.extra_flags {
            args.push(extra.clone());
        }
        Ok(args)
    }

    /// Argument vector for `cbld check`'s analysis pass over a C translation
    /// unit. Deliberately a smaller set than `c_flags`: no optimization
    /// level, no LTO, no sanitizers, no `extra_flags` — none of those affect
    /// what the analyzer parses or reports, and `--analyze` never reaches
    /// codegen. Standard and warnings still come from the profile, same as a
    /// real build, so analysis sees the same language dialect and enabled
    /// diagnostics.
    fn c_analyze_args(&self, source: &Path) -> Vec<String> {
        let p = &self.c_profile;
        let mut args = vec!["--analyze".to_string(), format!("-std={}", p.standard)];
        for w in &p.warnings {
            args.push(warning_flag(w));
        }
        self.push_diagnostics_and_includes(&mut args, &p.defines);
        args.push(source.to_string_lossy().to_string());
        args
    }

    /// C++ counterpart to `c_analyze_args` — only ever reads `cpp_profile`,
    /// same isolation guarantee as `cpp_args`/`cpp_flags`.
    fn cpp_analyze_args(&self, source: &Path) -> Vec<String> {
        let p = &self.cpp_profile;
        let mut args = vec!["--analyze".to_string(), format!("-std={}", p.standard)];
        if p.rtti {
            args.push("-frtti".to_string());
        } else {
            args.push("-fno-rtti".to_string());
        }
        if p.exceptions {
            args.push("-fexceptions".to_string());
        } else {
            args.push("-fno-exceptions".to_string());
        }
        for w in &p.warnings {
            args.push(warning_flag(w));
        }
        self.push_diagnostics_and_includes(&mut args, &p.defines);
        args.push(source.to_string_lossy().to_string());
        args
    }

    /// Compiler frontend flags for `clang-tidy -- <args>`. Language dialect,
    /// include paths, and defines — enough to parse, no codegen.
    pub fn tidy_frontend_args(&self, language: Language) -> Vec<String> {
        let mut args = match language {
            Language::C => {
                let p = &self.c_profile;
                vec![
                    "-x".to_string(),
                    "c".to_string(),
                    format!("-std={}", p.standard),
                ]
            }
            Language::Cpp => {
                let p = &self.cpp_profile;
                let mut args = vec![
                    "-x".to_string(),
                    "c++".to_string(),
                    format!("-std={}", p.standard),
                ];
                if p.rtti {
                    args.push("-frtti".to_string());
                } else {
                    args.push("-fno-rtti".to_string());
                }
                if p.exceptions {
                    args.push("-fexceptions".to_string());
                } else {
                    args.push("-fno-exceptions".to_string());
                }
                args
            }
        };
        let defines = match language {
            Language::C => &self.c_profile.defines,
            Language::Cpp => &self.cpp_profile.defines,
        };
        self.push_diagnostics_and_includes(&mut args, defines);
        args
    }

    /// Cache fingerprint for a package's complete language set. Prefixing each
    /// profile avoids collisions between equal-looking C and C++ flag vectors.
    pub fn cache_fingerprint_for(&self, languages: &[Language]) -> Result<Vec<String>> {
        let mut fingerprint = Vec::new();
        if languages.contains(&Language::C) {
            fingerprint.push("language=c".to_string());
            fingerprint.extend(self.c_flags()?);
        }
        if languages.contains(&Language::Cpp) {
            fingerprint.push("language=cpp".to_string());
            fingerprint.extend(self.cpp_flags()?);
        }
        Ok(fingerprint)
    }

    /// Flags common to both languages: includes, defines, debug/release shaping.
    ///
    /// `force_debug_syms` is true when the profile has at least one active
    /// sanitizer: sanitizer stack traces are unreadable raw addresses without
    /// `-g`, so cbld injects it even in a release/optimized profile that
    /// would otherwise strip symbols — and warns once when it has to override
    /// the profile's own choice.
    fn push_common(
        &self,
        args: &mut Vec<String>,
        profile_defines: &[String],
        force_debug_syms: bool,
    ) {
        self.push_diagnostics_and_includes(args, profile_defines);

        // `[package] ignore_warnings` / `--ignore-warnings`: silence every
        // warning. Emitted here, *after* the profile's `-W` groups (pushed by
        // `c_flags`/`cpp_flags` before this call), so `-w` overrides them —
        // but still before `extra_flags`, leaving that escape hatch the final
        // word. Deliberately absent from the `--analyze` path: suppressing
        // warnings there would defeat the point of `cbld check`.
        if self.ignore_warnings {
            args.push("-w".to_string());
        }

        // `cbld build --trace`: clang writes this unit's profile next to its
        // `-o` object path (same basename, `.json` extension) — see
        // trace.rs for the aggregation step that follows compilation.
        if self.trace {
            args.push("-ftime-trace".to_string());
        }

        if self.debug || force_debug_syms {
            args.push("-g".to_string());
        }
        if self.release {
            args.push("-DNDEBUG".to_string());
        }
    }

    /// The subset of flags shared by a real compile *and* a `cbld check`
    /// analysis pass: colorized machine-parseable diagnostics, the
    /// cross-compilation target (if any), include paths, and defines. Kept
    /// separate from `push_common` because analysis never wants `-g`,
    /// `-DNDEBUG`, or `-ftime-trace` — none of those affect what the
    /// analyzer reports, since `--analyze` never reaches codegen.
    fn push_diagnostics_and_includes(&self, args: &mut Vec<String>, profile_defines: &[String]) {
        // Emit machine-parseable diagnostics with caret context.
        args.push("-fcolor-diagnostics".to_string());
        args.push("-fno-caret-diagnostics".to_string());

        // `cbld build --target` / `[package] target`: cross-compile. Kept in
        // the shared helper (rather than only `push_common`) so `cbld
        // check --target` analyzes against the same target-specific
        // headers/macros a cross-compiled build would actually see.
        if let Some(target) = &self.target {
            args.push(format!("--target={target}"));
        }
        if let Some(sysroot) = &self.sysroot {
            args.push(format!("--sysroot={}", sysroot.display()));
        }

        for dir in &self.priority_include_dirs {
            args.push(format!("-I{}", dir.display()));
        }
        args.push(format!("-I{}", self.own_include_dir.display()));
        for dir in &self.include_dirs {
            args.push(format!("-I{}", dir.display()));
        }
        // Order: the profile's own `defines`, then project-wide `[package]
        // defines` (both languages), then the auto-generated feature defines.
        for def in profile_defines
            .iter()
            .chain(self.package_defines.iter())
            .chain(self.feature_defines.iter())
        {
            args.push(format!("-D{def}"));
        }
    }

    /// Build the final link command for an executable, or the ordered list of
    /// archiver candidates for a library.
    ///
    /// `objects` are the compiled object files; `output` is the artifact path
    /// (already extensioned correctly by the engine — `.o`/`.obj`, `.a`/`.lib`,
    /// with `.exe` on the executable side). `has_cpp` decides whether to link
    /// with the C++ driver (needed to pull in the C++ runtime/stdlib) — a
    /// concrete consequence of strict separation.
    ///
    /// `libs` are `[package] libs` names, each emitted as `-l<name>` after the
    /// output path so GNU ld's single-pass scan still sees object files first.
    /// They are ignored when `is_library` is true (the archiver does not take
    /// `-l` flags).
    ///
    /// Executables always resolve to exactly one command. Libraries resolve to
    /// one or more *candidates*, most-preferred first: the caller (`Engine`)
    /// is expected to try each in turn and only fall through to the next one
    /// if the program itself can't be spawned — the same "try, then fall
    /// back" shape used elsewhere in cbld (e.g. the resolver's clone retries).
    pub fn link_command(
        &self,
        objects: &[PathBuf],
        output: &Path,
        has_cpp: bool,
        is_library: bool,
        is_shared: bool,
        libs: &[String],
    ) -> Vec<LinkCommand> {
        self.link_command_with_flags(objects, output, has_cpp, is_library, is_shared, libs, &[])
    }

    /// Like [`Self::link_command`], with raw link arguments preserved after
    /// library names. This supports platform frameworks and linker directives
    /// that cannot be represented as `-l<name>`.
    #[allow(clippy::too_many_arguments)]
    pub fn link_command_with_flags(
        &self,
        objects: &[PathBuf],
        output: &Path,
        has_cpp: bool,
        is_library: bool,
        is_shared: bool,
        libs: &[String],
        link_flags: &[String],
    ) -> Vec<LinkCommand> {
        if is_library && !is_shared {
            return archiver_candidates(objects, output);
        }
        if is_shared {
            // Shared library via linker with -shared and -fPIC.
            let (driver, profile_lto, profile_sanitizers) = if has_cpp {
                (
                    Language::Cpp.driver(),
                    self.cpp_profile.lto,
                    &self.cpp_profile.sanitizers,
                )
            } else {
                (
                    Language::C.driver(),
                    self.c_profile.lto,
                    &self.c_profile.sanitizers,
                )
            };
            let sanitizers = parse_sanitizers(profile_sanitizers).unwrap_or_default();
            let mut args = Vec::new();
            if let Some(target) = &self.target {
                args.push(format!("--target={target}"));
            }
            if let Some(sysroot) = &self.sysroot {
                args.push(format!("--sysroot={}", sysroot.display()));
            }
            args.push("-shared".to_string());
            args.push("-fPIC".to_string());
            if profile_lto {
                args.push("-flto".to_string());
            }
            for s in &sanitizers {
                args.push(s.flag().to_string());
            }
            for o in objects {
                args.push(o.to_string_lossy().to_string());
            }
            args.push("-o".to_string());
            args.push(output.to_string_lossy().to_string());
            for lib in libs {
                if lib.is_empty() {
                    continue;
                }
                if lib.starts_with('-') {
                    args.push(lib.clone());
                } else {
                    args.push(format!("-l{lib}"));
                }
            }
            args.extend(link_flags.iter().cloned());
            return vec![LinkCommand {
                program: driver.to_string(),
                args,
            }];
        }

        // A mixed C/C++ package links with the C++ driver when any C++ unit is
        // present, so the C++ runtime is available. Its link profile owns the
        // sanitizer/LTO flags for that final driver invocation.
        let (driver, profile_lto, profile_sanitizers) = if has_cpp {
            (
                Language::Cpp.driver(),
                self.cpp_profile.lto,
                &self.cpp_profile.sanitizers,
            )
        } else {
            (
                Language::C.driver(),
                self.c_profile.lto,
                &self.c_profile.sanitizers,
            )
        };
        // Already validated in `Compiler::validate()` before any compilation
        // started, so parsing here can only fail if that check was skipped.
        let sanitizers = parse_sanitizers(profile_sanitizers).unwrap_or_default();

        let mut args = Vec::new();
        // Cross-compilation: the linked artifact must agree with the object
        // files it's linking, or clang's default (host) linker invocation
        // will reject them as the wrong architecture/ABI.
        if let Some(target) = &self.target {
            args.push(format!("--target={target}"));
        }
        if let Some(sysroot) = &self.sysroot {
            args.push(format!("--sysroot={}", sysroot.display()));
        }
        if profile_lto {
            args.push("-flto".to_string());
        }
        for s in &sanitizers {
            args.push(s.flag().to_string());
        }
        for o in objects {
            args.push(o.to_string_lossy().to_string());
        }
        args.push("-o".to_string());
        args.push(output.to_string_lossy().to_string());
        for lib in libs {
            if lib.is_empty() {
                continue;
            }
            if lib.starts_with('-') {
                args.push(lib.clone());
            } else {
                args.push(format!("-l{lib}"));
            }
        }
        args.extend(link_flags.iter().cloned());
        vec![LinkCommand {
            program: driver.to_string(),
            args,
        }]
    }
}

/// GNU `ar` supports `rcsD` (deterministic zero index); BSD `ar` on macOS only
/// accepts `rcs`.
fn unix_ar_create_mode() -> &'static str {
    if cfg!(target_os = "macos") {
        "rcs"
    } else {
        "rcsD"
    }
}

/// Static-archive command candidates, most preferred first.
///
/// On Linux, `ar rcsD <archive> <objects...>` (GNU). On macOS, BSD `ar` uses
/// `rcs` instead; `llvm-ar` (when present) still accepts `rcsD`. Windows has
/// two incompatible ones for the same job — `llvm-ar` with the Unix-style form,
/// while MSVC's `lib.exe` wants `/OUT:<archive> <objects...>` instead. Rather
/// than guessing which toolchain is installed, both Windows candidates are
/// returned in preference order and `Engine::run_link` tries each in turn,
/// only advancing past a candidate when the program itself isn't found.
fn archiver_candidates(objects: &[PathBuf], output: &Path) -> Vec<LinkCommand> {
    let out = output.to_string_lossy().to_string();
    let objs: Vec<String> = objects
        .iter()
        .map(|o| o.to_string_lossy().to_string())
        .collect();

    if cfg!(target_os = "windows") {
        let mut llvm_ar_args = vec!["rcsD".to_string(), out.clone()];
        llvm_ar_args.extend(objs.iter().cloned());

        let mut lib_args = vec![format!("/OUT:{out}")];
        lib_args.extend(objs);

        return vec![
            LinkCommand {
                program: "llvm-ar".to_string(),
                args: llvm_ar_args,
            },
            LinkCommand {
                program: "lib.exe".to_string(),
                args: lib_args,
            },
        ];
    }

    if cfg!(target_os = "macos") {
        let mut llvm_ar_args = vec!["rcsD".to_string(), out.clone()];
        llvm_ar_args.extend(objs.iter().cloned());

        let mut ar_args = vec![unix_ar_create_mode().to_string(), out];
        ar_args.extend(objs);

        return vec![
            LinkCommand {
                program: "llvm-ar".to_string(),
                args: llvm_ar_args,
            },
            LinkCommand {
                program: "ar".to_string(),
                args: ar_args,
            },
        ];
    }

    let mut args = vec![unix_ar_create_mode().to_string(), out];
    args.extend(objs);
    vec![LinkCommand {
        program: "ar".to_string(),
        args,
    }]
}

/// A resolved link/archive command.
#[derive(Debug, Clone)]
pub struct LinkCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiler() -> Compiler {
        Compiler::new(
            CProfile::default(),
            CppProfile::default(),
            Path::new("."),
            Vec::new(),
            &[],
            false,
            false,
            None,
            Vec::new(),
            false,
        )
    }

    #[test]
    fn driver_command_splits_multiword_drivers() {
        let cmd = driver_command("zig cc");
        assert_eq!(cmd.get_program(), "zig");
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["cc"]);
    }

    /// `link_command` for an executable always resolves to exactly one
    /// candidate, driven by whichever language pulled in the unit (the C++
    /// driver if any translation unit was C++, to pull in the C++ runtime).
    #[test]
    fn link_command_executable_picks_driver_by_language() {
        let c = compiler();
        let objects = vec![PathBuf::from("main.o")];
        let output = PathBuf::from("app");

        let c_cmds = c.link_command(&objects, &output, false, false, false, &[]);
        assert_eq!(c_cmds.len(), 1);
        assert_eq!(c_cmds[0].program, Language::C.driver());

        let cpp_cmds = c.link_command(&objects, &output, true, false, false, &[]);
        assert_eq!(cpp_cmds.len(), 1);
        assert_eq!(cpp_cmds[0].program, Language::Cpp.driver());
    }

    /// The archiver fallback chain is platform-specific: Unix has exactly one
    /// candidate (`ar`), Windows offers `llvm-ar` first and falls back to
    /// MSVC's `lib.exe`, with each program's own calling convention intact.
    #[cfg(target_os = "windows")]
    #[test]
    fn link_command_library_windows_fallback_chain() {
        let c = compiler();
        let objects = vec![PathBuf::from("a.obj"), PathBuf::from("b.obj")];
        let output = PathBuf::from("mylib.lib");

        let cmds = c.link_command(&objects, &output, false, true, false, &[]);
        assert_eq!(cmds.len(), 2);

        assert_eq!(cmds[0].program, "llvm-ar");
        assert_eq!(cmds[0].args[0], "rcsD");
        assert_eq!(cmds[0].args[1], "mylib.lib");
        assert!(cmds[0].args.contains(&"a.obj".to_string()));
        assert!(cmds[0].args.contains(&"b.obj".to_string()));

        assert_eq!(cmds[1].program, "lib.exe");
        assert_eq!(cmds[1].args[0], "/OUT:mylib.lib");
        assert!(cmds[1].args.contains(&"a.obj".to_string()));
        assert!(cmds[1].args.contains(&"b.obj".to_string()));
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    #[test]
    fn link_command_library_unix_single_candidate() {
        let c = compiler();
        let objects = vec![PathBuf::from("a.o"), PathBuf::from("b.o")];
        let output = PathBuf::from("libmy.a");

        let cmds = c.link_command(&objects, &output, false, true, false, &[]);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].program, "ar");
        assert_eq!(cmds[0].args[0], "rcsD");
        assert_eq!(cmds[0].args[1], "libmy.a");
        assert!(cmds[0].args.contains(&"a.o".to_string()));
        assert!(cmds[0].args.contains(&"b.o".to_string()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn link_command_library_macos_fallback_chain() {
        let c = compiler();
        let objects = vec![PathBuf::from("a.o"), PathBuf::from("b.o")];
        let output = PathBuf::from("libmy.a");

        let cmds = c.link_command(&objects, &output, false, true, false, &[]);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].program, "llvm-ar");
        assert_eq!(cmds[0].args[0], "rcsD");
        assert_eq!(cmds[1].program, "ar");
        assert_eq!(cmds[1].args[0], "rcs");
    }

    /// Object/archive extensions are decided by the engine when it builds the
    /// `output`/`objects` paths handed to `link_command`, not by this
    /// function itself — verify the extension convention the rest of the
    /// pipeline relies on.
    #[test]
    fn object_and_archive_extensions_match_platform_convention() {
        if cfg!(target_os = "windows") {
            assert_eq!(Path::new("main.obj").extension().unwrap(), "obj");
            assert_eq!(Path::new("mylib.lib").extension().unwrap(), "lib");
        } else {
            assert_eq!(Path::new("main.o").extension().unwrap(), "o");
            assert_eq!(Path::new("libmy.a").extension().unwrap(), "a");
        }
    }

    /// The cache fingerprint must change when a flag-affecting profile field
    /// changes, and must never embed any source/object path (it has to be
    /// portable across machines/checkouts for the global cache to be useful).
    #[test]
    fn cache_fingerprint_excludes_paths_and_reflects_profile_changes() {
        let baseline = compiler();
        let fp_debug = baseline.cache_fingerprint_for(&[Language::C]).unwrap();
        assert!(!fp_debug
            .iter()
            .any(|f| f.contains(".c") || f.contains(".o")));

        let release_profile = CProfile {
            optimization: "0".to_string(),
            ..CProfile::default()
        };
        let release = Compiler::new(
            release_profile,
            CppProfile::default(),
            Path::new("."),
            Vec::new(),
            &[],
            true,
            false,
            None,
            Vec::new(),
            false,
        );
        let fp_release = release.cache_fingerprint_for(&[Language::C]).unwrap();

        assert_ne!(fp_debug, fp_release);
    }

    #[test]
    fn mixed_language_cache_fingerprint_includes_both_profiles() {
        let c = Compiler::new(
            CProfile {
                defines: vec!["C_ONLY".to_string()],
                ..CProfile::default()
            },
            CppProfile {
                defines: vec!["CPP_ONLY".to_string()],
                ..CppProfile::default()
            },
            Path::new("."),
            Vec::new(),
            &[],
            false,
            false,
            None,
            Vec::new(),
            false,
        );
        let c_only = c.cache_fingerprint_for(&[Language::C]).unwrap();
        let mixed = c
            .cache_fingerprint_for(&[Language::C, Language::Cpp])
            .unwrap();
        assert!(mixed.contains(&"language=c".to_string()));
        assert!(mixed.contains(&"language=cpp".to_string()));
        assert!(mixed.contains(&"-DC_ONLY".to_string()));
        assert!(mixed.contains(&"-DCPP_ONLY".to_string()));
        assert_ne!(c_only, mixed);
    }

    #[test]
    fn language_from_extension_is_strictly_separated() {
        assert_eq!(
            Language::from_extension(Path::new("foo.c")),
            Some(Language::C)
        );
        for ext in [
            "cc", "cpp", "cxx", "c++", "cp", "cppm", "ccm", "cxxm", "c++m", "ixx", "mxx",
        ] {
            assert_eq!(
                Language::from_extension(Path::new(&format!("foo.{ext}"))),
                Some(Language::Cpp)
            );
        }
        assert_eq!(Language::from_extension(Path::new("foo.h")), None);
        assert!(Language::is_header(Path::new("foo.h")));
        assert!(Language::is_header(Path::new("foo.hpp")));
        assert!(!Language::is_header(Path::new("foo.c")));
    }

    #[test]
    fn sysroot_reaches_compile_and_link_commands() {
        let c = Compiler::new_with_sysroot(
            CProfile::default(),
            CppProfile::default(),
            Path::new("."),
            Vec::new(),
            &[],
            false,
            false,
            Some("aarch64-unknown-linux-gnu".into()),
            Some(PathBuf::from("/opt/aarch64-sysroot")),
            Vec::new(),
            false,
        );
        let unit = c
            .compile_unit(Path::new("main.cpp"), Path::new("main.o"))
            .unwrap();
        assert!(unit
            .args
            .contains(&"--target=aarch64-unknown-linux-gnu".to_string()));
        assert!(unit
            .args
            .contains(&"--sysroot=/opt/aarch64-sysroot".to_string()));

        let links = c.link_command(
            &[PathBuf::from("main.o")],
            Path::new("app"),
            true,
            false,
            false,
            &[],
        );
        assert!(links[0]
            .args
            .contains(&"--sysroot=/opt/aarch64-sysroot".to_string()));
    }

    /// A capital `.C` is C++ (Unix tradition), and must not collapse into the
    /// lowercase `.c` = C arm — otherwise a legacy C++ file would be compiled
    /// with the C driver and mis-routed at link time.
    #[test]
    fn capital_c_extension_routes_to_cpp() {
        assert_eq!(
            Language::from_extension(Path::new("Widget.C")),
            Some(Language::Cpp)
        );
        assert_eq!(
            Language::from_extension(Path::new("Widget.C"))
                .unwrap()
                .driver(),
            "clang++"
        );
        // Lowercase `.c` stays C.
        assert_eq!(
            Language::from_extension(Path::new("widget.c")),
            Some(Language::C)
        );
    }

    /// `[package] ignore_warnings` (and `--ignore-warnings`, which sets the
    /// same flag) must inject a single `-w` into a real compile — after the
    /// profile's `-W` groups so it wins — and never into the `--analyze`
    /// path, where suppressing warnings would defeat `cbld check`.
    #[test]
    fn ignore_warnings_injects_dash_w_into_compile_but_not_analyze() {
        let plain = compiler();
        assert!(!plain.c_flags().unwrap().contains(&"-w".to_string()));

        let c = Compiler::new(
            CProfile {
                warnings: vec!["all".to_string(), "error".to_string()],
                ..CProfile::default()
            },
            CppProfile::default(),
            Path::new("."),
            Vec::new(),
            &[],
            false,
            false,
            None,
            Vec::new(),
            true, // ignore_warnings
        );
        let flags = c.c_flags().unwrap();
        let w_pos = flags.iter().position(|a| a == "-w").expect("-w present");
        let wall_pos = flags
            .iter()
            .position(|a| a == "-Wall")
            .expect("-Wall present");
        assert!(wall_pos < w_pos, "-w must come after -Wall to override it");
        assert!(c.cpp_flags().unwrap().contains(&"-w".to_string()));

        // Analysis pass never suppresses warnings.
        let unit = c.analyze_unit(Path::new("src/main.c")).unwrap();
        assert!(!unit.args.contains(&"-w".to_string()));
    }

    /// `[package] defines` become `-D<entry>` for *both* languages, on top of
    /// each profile's own defines, and must change the cache fingerprint (they
    /// affect codegen).
    #[test]
    fn package_defines_reach_both_languages_and_the_fingerprint() {
        let c = Compiler::new(
            CProfile::default(),
            CppProfile::default(),
            Path::new("."),
            Vec::new(),
            &[],
            false,
            false,
            None,
            vec!["LEGACY".to_string(), "VERSION=2".to_string()],
            false,
        );
        for flags in [c.c_flags().unwrap(), c.cpp_flags().unwrap()] {
            assert!(flags.contains(&"-DLEGACY".to_string()));
            assert!(flags.contains(&"-DVERSION=2".to_string()));
        }
        assert_ne!(
            compiler().c_flags().unwrap(),
            c.c_flags().unwrap(),
            "package defines must alter the flags/fingerprint"
        );
    }

    #[test]
    fn sanitizer_parse_accepts_all_four_and_rejects_unknown() {
        assert_eq!(Sanitizer::parse("address").unwrap(), Sanitizer::Address);
        assert_eq!(Sanitizer::parse("thread").unwrap(), Sanitizer::Thread);
        assert_eq!(Sanitizer::parse("undefined").unwrap(), Sanitizer::Undefined);
        assert_eq!(Sanitizer::parse("leak").unwrap(), Sanitizer::Leak);
        assert!(Sanitizer::parse("bogus").is_err());
    }

    fn compiler_with(c: CProfile) -> Compiler {
        Compiler::new(
            c,
            CppProfile::default(),
            Path::new("."),
            Vec::new(),
            &[],
            false,
            false,
            None,
            Vec::new(),
            false,
        )
    }

    /// LTO combined with ASan or LSan must abort validation with a clear
    /// explanation, since link-time optimization and these sanitizers are
    /// mutually exclusive.
    #[test]
    fn validate_rejects_lto_with_address_or_leak_sanitizer() {
        let c = compiler_with(CProfile {
            lto: true,
            sanitizers: vec!["address".to_string()],
            ..CProfile::default()
        });
        assert!(c.validate().is_err());

        let c = compiler_with(CProfile {
            lto: true,
            sanitizers: vec!["leak".to_string()],
            ..CProfile::default()
        });
        assert!(c.validate().is_err());

        // LTO alone, or LTO with an unrelated sanitizer, is fine.
        let c = compiler_with(CProfile {
            lto: true,
            sanitizers: vec!["undefined".to_string()],
            ..CProfile::default()
        });
        assert!(c.validate().is_ok());
    }

    /// TSan can never coexist with ASan/LSan in the same binary — their
    /// runtime interceptors conflict.
    #[test]
    fn validate_rejects_thread_with_address_or_leak_sanitizer() {
        let c = compiler_with(CProfile {
            sanitizers: vec!["thread".to_string(), "address".to_string()],
            ..CProfile::default()
        });
        assert!(c.validate().is_err());

        let c = compiler_with(CProfile {
            sanitizers: vec!["thread".to_string(), "leak".to_string()],
            ..CProfile::default()
        });
        assert!(c.validate().is_err());

        let c = compiler_with(CProfile {
            sanitizers: vec!["thread".to_string(), "undefined".to_string()],
            ..CProfile::default()
        });
        assert!(c.validate().is_ok());
    }

    /// An unrecognized sanitizer name fails validation up front, same as an
    /// unrecognized optimization level.
    #[test]
    fn validate_rejects_unknown_sanitizer_name() {
        let c = compiler_with(CProfile {
            sanitizers: vec!["valgrind".to_string()],
            ..CProfile::default()
        });
        assert!(c.validate().is_err());
    }

    /// An empty `sanitizers` array (the default) must behave exactly like a
    /// v0.3.0 manifest that never mentions sanitizers at all: no `-g`
    /// injection beyond the profile's own debug/release setting, and no
    /// `-fsanitize=` flags anywhere in the compile args.
    #[test]
    fn empty_sanitizers_is_backwards_compatible_with_v0_3_0() {
        let c = compiler();
        let args = c.c_flags().unwrap();
        assert!(!args.iter().any(|a| a.starts_with("-fsanitize=")));
        assert!(!args.contains(&"-flto".to_string()));
    }

    /// Active sanitizers must append `-g` and the matching `-fsanitize=`
    /// flags to the compile args, strictly before any user `extra_flags`, so
    /// power users can still append granular sub-flags afterward.
    #[test]
    fn active_sanitizers_inject_debug_symbols_and_precede_extra_flags() {
        let c = compiler_with(CProfile {
            sanitizers: vec!["address".to_string(), "undefined".to_string()],
            extra_flags: vec!["-fno-omit-frame-pointer".to_string()],
            ..CProfile::default()
        });
        let args = c.c_flags().unwrap();

        assert!(args.contains(&"-g".to_string()));
        let asan_pos = args
            .iter()
            .position(|a| a == "-fsanitize=address")
            .expect("asan flag present");
        let ubsan_pos = args
            .iter()
            .position(|a| a == "-fsanitize=undefined")
            .expect("ubsan flag present");
        let extra_pos = args
            .iter()
            .position(|a| a == "-fno-omit-frame-pointer")
            .expect("extra flag present");
        assert!(asan_pos < extra_pos);
        assert!(ubsan_pos < extra_pos);
    }

    /// The exact same `-fsanitize=` flags compiled into the object files must
    /// also reach the final link command, and must precede the object files
    /// in the argument vector.
    #[test]
    fn link_command_propagates_matching_sanitizer_flags() {
        let c = compiler_with(CProfile {
            sanitizers: vec!["address".to_string()],
            ..CProfile::default()
        });
        let objects = vec![PathBuf::from("main.o")];
        let output = PathBuf::from("app");

        let cmds = c.link_command(&objects, &output, false, false, false, &[]);
        assert_eq!(cmds.len(), 1);
        let sanitize_pos = cmds[0]
            .args
            .iter()
            .position(|a| a == "-fsanitize=address")
            .expect("asan flag present at link time");
        let object_pos = cmds[0]
            .args
            .iter()
            .position(|a| a == "main.o")
            .expect("object file present");
        assert!(sanitize_pos < object_pos);
    }

    /// `cbld build --trace` must inject `-ftime-trace` into every
    /// compile — and only when explicitly requested, matching the
    /// backwards-compatibility guarantee already covered by
    /// `empty_sanitizers_is_backwards_compatible_with_v0_3_0`.
    #[test]
    fn trace_flag_injects_ftime_trace_only_when_requested() {
        let without_trace = compiler();
        assert!(!without_trace
            .c_flags()
            .unwrap()
            .contains(&"-ftime-trace".to_string()));

        let with_trace = Compiler::new(
            CProfile::default(),
            CppProfile::default(),
            Path::new("."),
            Vec::new(),
            &[],
            false,
            true,
            None,
            Vec::new(),
            false,
        );
        assert!(with_trace
            .c_flags()
            .unwrap()
            .contains(&"-ftime-trace".to_string()));
        assert!(with_trace
            .cpp_flags()
            .unwrap()
            .contains(&"-ftime-trace".to_string()));
    }

    fn compiler_with_target(target: &str) -> Compiler {
        Compiler::new(
            CProfile::default(),
            CppProfile::default(),
            Path::new("."),
            Vec::new(),
            &[],
            false,
            false,
            Some(target.to_string()),
            Vec::new(),
            false,
        )
    }

    /// `--target=<triple>` must be injected into both compile phases, and
    /// must be absent entirely when no target was configured — the default,
    /// native-build case must stay byte-for-byte unchanged when unset.
    #[test]
    fn target_flag_injects_into_compile_args_only_when_set() {
        assert!(!compiler()
            .c_flags()
            .unwrap()
            .iter()
            .any(|a| a.starts_with("--target=")));

        let cross = compiler_with_target("aarch64-unknown-linux-gnu");
        assert!(cross
            .c_flags()
            .unwrap()
            .contains(&"--target=aarch64-unknown-linux-gnu".to_string()));
        assert!(cross
            .cpp_flags()
            .unwrap()
            .contains(&"--target=aarch64-unknown-linux-gnu".to_string()));
    }

    /// The same `--target=` flag compiled into the object files must also
    /// reach the final link step, and must precede the object files in the
    /// argument vector — mirroring
    /// `link_command_propagates_matching_sanitizer_flags`.
    #[test]
    fn target_flag_propagates_to_link_command_but_not_archiver() {
        let cross = compiler_with_target("wasm32-unknown-unknown");
        let objects = vec![PathBuf::from("main.o")];

        let exe_cmds =
            cross.link_command(&objects, &PathBuf::from("app"), false, false, false, &[]);
        assert_eq!(exe_cmds.len(), 1);
        let target_pos = exe_cmds[0]
            .args
            .iter()
            .position(|a| a == "--target=wasm32-unknown-unknown")
            .expect("target flag present at link time");
        let object_pos = exe_cmds[0]
            .args
            .iter()
            .position(|a| a == "main.o")
            .expect("object file present");
        assert!(target_pos < object_pos);

        let lib_output = if cfg!(target_os = "windows") {
            PathBuf::from("mylib.lib")
        } else {
            PathBuf::from("libmy.a")
        };
        let lib_cmds = cross.link_command(&objects, &lib_output, false, true, false, &[]);
        for cmd in &lib_cmds {
            assert!(!cmd.args.iter().any(|a| a.starts_with("--target=")));
        }
    }

    /// `cbld check` must swap `-c -o <obj>` for `--analyze`, keep the
    /// profile's warnings/standard/includes/target, and drop everything
    /// that only matters for real codegen (optimization, LTO, sanitizers,
    /// `-g`/`-DNDEBUG`, `-ftime-trace`, `extra_flags`).
    #[test]
    fn analyze_unit_uses_analyze_flag_and_drops_codegen_only_flags() {
        let c = Compiler::new(
            CProfile {
                warnings: vec!["all".to_string(), "extra".to_string()],
                sanitizers: vec!["address".to_string()],
                lto: true,
                extra_flags: vec!["-Wpadded".to_string()],
                ..CProfile::default()
            },
            CppProfile::default(),
            Path::new("."),
            Vec::new(),
            &[],
            false,
            false,
            Some("aarch64-unknown-linux-gnu".to_string()),
            Vec::new(),
            false,
        );
        let unit = c.analyze_unit(Path::new("src/main.c")).unwrap();

        assert_eq!(unit.args[0], "--analyze");
        assert!(!unit.args.contains(&"-c".to_string()));
        assert!(!unit.args.iter().any(|a| a == "-o"));
        assert!(unit.args.contains(&"-Wall".to_string()));
        assert!(unit.args.contains(&"-Wextra".to_string()));
        assert!(unit
            .args
            .contains(&"--target=aarch64-unknown-linux-gnu".to_string()));
        assert!(!unit.args.iter().any(|a| a.starts_with("-O")));
        assert!(!unit.args.contains(&"-flto".to_string()));
        assert!(!unit.args.iter().any(|a| a.starts_with("-fsanitize=")));
        assert!(!unit.args.contains(&"-g".to_string()));
        assert!(!unit.args.contains(&"-ftime-trace".to_string()));
        assert!(!unit.args.contains(&"-Wpadded".to_string()));
        assert_eq!(unit.args.last().unwrap(), "src/main.c");
    }

    #[test]
    fn tidy_frontend_args_parse_only_no_analyze_or_codegen() {
        let c = Compiler::new(
            CProfile::default(),
            CppProfile {
                standard: "c++20".to_string(),
                rtti: false,
                exceptions: true,
                ..CppProfile::default()
            },
            Path::new("."),
            vec![PathBuf::from("deps/include")],
            &[],
            false,
            false,
            None,
            Vec::new(),
            false,
        );
        let args = c.tidy_frontend_args(Language::Cpp);
        assert!(args.contains(&"-x".to_string()));
        assert!(args.contains(&"c++".to_string()));
        assert!(args.contains(&"-std=c++20".to_string()));
        assert!(args.contains(&"-fno-rtti".to_string()));
        assert!(args.contains(&"-fexceptions".to_string()));
        assert!(!args.contains(&"--analyze".to_string()));
        assert!(!args.contains(&"-c".to_string()));
        assert!(args.iter().any(|a| a.contains("deps/include")));
    }

    /// Library builds go through the archiver, not the linker — sanitizer
    /// flags are meaningless there and must not appear.
    #[test]
    fn link_command_library_has_no_sanitizer_flags() {
        let c = compiler_with(CProfile {
            sanitizers: vec!["address".to_string()],
            ..CProfile::default()
        });
        let objects = vec![PathBuf::from("a.o")];
        let output = if cfg!(target_os = "windows") {
            PathBuf::from("mylib.lib")
        } else {
            PathBuf::from("libmy.a")
        };

        let cmds = c.link_command(&objects, &output, false, true, false, &[]);
        for cmd in &cmds {
            assert!(!cmd.args.iter().any(|a| a.starts_with("-fsanitize=")));
        }
    }

    #[test]
    fn link_command_executable_appends_system_libs() {
        let c = compiler();
        let objects = vec![PathBuf::from("main.o")];
        let output = PathBuf::from("app");
        let libs = ["pthread".to_string(), "-lm".to_string()];
        let cmds = c.link_command(&objects, &output, false, false, false, &libs);
        assert_eq!(cmds.len(), 1);
        let args = &cmds[0].args;
        let o_pos = args.iter().position(|a| a == "-o").expect("-o");
        assert!(args[o_pos + 1] == "app");
        assert_eq!(args[args.len() - 2], "-lpthread");
        assert_eq!(args[args.len() - 1], "-lm");

        let lib_output = if cfg!(target_os = "windows") {
            PathBuf::from("mylib.lib")
        } else {
            PathBuf::from("libmy.a")
        };
        let lib_cmds = c.link_command(&objects, &lib_output, false, true, false, &libs);
        for cmd in &lib_cmds {
            assert!(!cmd.args.iter().any(|a| a.starts_with("-l")));
        }
    }

    #[test]
    fn link_command_preserves_raw_platform_flags_after_libraries() {
        let c = compiler();
        let commands = c.link_command_with_flags(
            &[PathBuf::from("main.o")],
            &PathBuf::from("app"),
            false,
            false,
            false,
            &["dl".to_string()],
            &["-framework".to_string(), "Cocoa".to_string()],
        );
        assert_eq!(
            &commands[0].args[commands[0].args.len() - 2..],
            ["-framework", "Cocoa"].as_slice()
        );
        assert!(commands[0].args.contains(&"-ldl".to_string()));
    }
}
