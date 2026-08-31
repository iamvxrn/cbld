//! cbld — entry point.
//!
//! `main` is a thin bridge: parse the CLI, dispatch to a handler, and turn a
//! `CbldError` into a clean, non-panicking process exit. All real logic lives
//! in the dedicated modules.

mod bench;
mod cli;
mod compdb;
mod compiler;
mod doctor;
mod engine;
mod error;
mod fmt;
mod glob;
mod hash;
mod json;
mod lint;
mod manifest;
mod migrate;
mod recipe;
mod resolver;
mod test;
mod trace;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use cli::{
    BenchArgs, BuildArgs, CheckArgs, Cli, Command as Cmd, FmtArgs, InitArgs, LintArgs, RunArgs,
    TestArgs, UpdateArgs, VendorArgs,
};
use compiler::Compiler;
use engine::{default_jobs, require_package, BuildDest, Crate, Engine, Layout, ScanConfig};
use error::{CbldError, IoPathExt, Result};
use json::Json;
use manifest::{Dependency, Lockfile, Manifest, Package, ToolchainSpec};
use recipe::PackageIndex;
use resolver::{build_lockfile, package_name, ResolvedDep, Resolver};

fn main() {
    let cli = Cli::parse_args();
    let verbose = cli.verbose > 0;
    let quiet = cli.quiet;
    let json = cli.json;

    let result = match cli.command {
        Cmd::Build(args) => cmd_build_top_level(args, verbose, quiet, json),
        Cmd::Run(args) => cmd_run(args, verbose, quiet, json),
        Cmd::Init(args) => cmd_init(args, quiet, json),
        Cmd::Update(args) => cmd_update(args, verbose, quiet, json),
        Cmd::Doctor => doctor::run(verbose, json),
        Cmd::Sync => cmd_sync(verbose, quiet, json),
        Cmd::Migrate(args) => cmd_migrate(args, quiet, json),
        Cmd::Vendor(args) => cmd_vendor(args, verbose, quiet, json),
        Cmd::Check(args) => cmd_check(args, verbose, quiet, json),
        Cmd::Fmt(args) => cmd_fmt(args, verbose, quiet, json),
        Cmd::Lint(args) => cmd_lint(args, verbose, quiet, json),
        Cmd::Test(args) => cmd_test(args, verbose, quiet, json),
        Cmd::Bench(args) => cmd_bench(args, verbose, quiet, json),
        Cmd::Completions(args) => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            clap_complete::generate(args.shell, &mut cmd, "cbld", &mut std::io::stdout());
            Ok(())
        }
    };

    if let Err(err) = result {
        if json {
            // JSON already emitted by the command wrapper (or by build_top_level).
            // Don't duplicate human diagnostics.
            std::process::exit(1);
        }
        eprintln!("\x1b[1;31merror\x1b[0m: {err}");
        // Print the cause chain for deeper context.
        let mut source = std::error::Error::source(&err);
        while let Some(cause) = source {
            eprintln!("  \x1b[2mcaused by:\x1b[0m {cause}");
            source = cause.source();
        }
        std::process::exit(1);
    }
}

/// Outcome of a successful build, reused by `run`.
struct BuildOutcome {
    artifact: PathBuf,
    crate_kind: Crate,
    /// How many library packages (this build's dependencies and/or the root
    /// package itself) were served from the global build cache instead of
    /// being recompiled.
    cache_hits: usize,
    /// One `compile_commands.json` entry per translation unit compiled in
    /// this invocation (root package plus every dependency), written to the
    /// project root by `cmd_build` after a successful build.
    compile_commands: Vec<compdb::CompileCommandEntry>,
}

/// Include dirs, archives, cache hits, and compile-commands entries from
/// building the resolved dependency graph. A named struct so clippy doesn't
/// trip on the four-tuple return of `build_dependencies`.
struct DepArtifacts {
    includes: Vec<PathBuf>,
    archives: Vec<PathBuf>,
    pkg_libs: Vec<String>,
    cache_hits: usize,
    compile_commands: Vec<compdb::CompileCommandEntry>,
}

/// Top-level `cbld build` entry point.
fn cmd_build_top_level(args: BuildArgs, verbose: bool, quiet: bool, json: bool) -> Result<()> {
    if !json {
        return build_with_diagnostics(args, verbose, quiet, json).map(|_| ());
    }

    let started = Instant::now();
    let result = build_with_diagnostics(args, verbose, true, true);
    let duration_ms = started.elapsed().as_millis() as i64;

    let payload = match &result {
        Ok(outcome) => build_success_payload(outcome, duration_ms),
        Err(err) => build_failure_payload(err, duration_ms),
    };
    println!("{}", payload.render());

    result.map(|_| ())
}

/// `{"status":"success","duration_ms":N,"cache_hits":N,"artifact":"...","errors":[]}`
fn build_success_payload(outcome: &BuildOutcome, duration_ms: i64) -> Json {
    Json::Object(vec![
        ("status".to_string(), Json::str("success")),
        ("duration_ms".to_string(), Json::Number(duration_ms)),
        (
            "cache_hits".to_string(),
            Json::Number(outcome.cache_hits as i64),
        ),
        (
            "artifact".to_string(),
            Json::str(outcome.artifact.display().to_string()),
        ),
        ("errors".to_string(), Json::Array(Vec::new())),
    ])
}

/// `{"status":"failure","duration_ms":N,"cache_hits":0,"errors":[{...}]}` —
/// `errors` carries the structured `CompileDiagnostic`s from
/// `CbldError::Compilation` when available, or a single synthetic entry
/// built from the error's `Display` text otherwise (e.g. a layout violation
/// that never reached the compiler at all).
fn build_failure_payload(err: &CbldError, duration_ms: i64) -> Json {
    let errors: Vec<Json> = match err {
        CbldError::Compilation { diagnostics, .. } => diagnostics
            .iter()
            .map(|d| {
                Json::Object(vec![
                    ("file".to_string(), Json::str(d.file.display().to_string())),
                    ("line".to_string(), Json::Number(d.line as i64)),
                    ("column".to_string(), Json::Number(d.column as i64)),
                    ("severity".to_string(), Json::str(d.severity)),
                    ("message".to_string(), Json::str(d.message.clone())),
                ])
            })
            .collect(),
        other => vec![Json::Object(vec![
            ("file".to_string(), Json::Null),
            ("line".to_string(), Json::Number(0)),
            ("column".to_string(), Json::Number(0)),
            ("severity".to_string(), Json::str("error")),
            ("message".to_string(), Json::str(other.to_string())),
        ])],
    };

    Json::Object(vec![
        ("status".to_string(), Json::str("failure")),
        ("duration_ms".to_string(), Json::Number(duration_ms)),
        ("cache_hits".to_string(), Json::Number(0)),
        ("errors".to_string(), Json::Array(errors)),
    ])
}

struct TestOutcome {
    passed: usize,
    failed: usize,
    binary: PathBuf,
}

fn cmd_test(args: TestArgs, verbose: bool, quiet: bool, json: bool) -> Result<()> {
    let started = Instant::now();
    let result = run_tests_command(args, verbose, if json { true } else { quiet });
    if json {
        let duration_ms = started.elapsed().as_millis() as i64;
        let payload = match &result {
            Ok(outcome) => Json::Object(vec![
                ("status".to_string(), Json::str("success")),
                ("duration_ms".to_string(), Json::Number(duration_ms)),
                ("passed".to_string(), Json::Number(outcome.passed as i64)),
                ("failed".to_string(), Json::Number(outcome.failed as i64)),
                (
                    "binary".to_string(),
                    Json::str(outcome.binary.display().to_string()),
                ),
                ("errors".to_string(), Json::Array(Vec::new())),
            ]),
            Err(err) => Json::Object(vec![
                ("status".to_string(), Json::str("failure")),
                ("duration_ms".to_string(), Json::Number(duration_ms)),
                ("passed".to_string(), Json::Number(0)),
                ("failed".to_string(), Json::Number(0)),
                (
                    "errors".to_string(),
                    Json::Array(vec![Json::Object(vec![
                        ("severity".to_string(), Json::str("error")),
                        ("message".to_string(), Json::str(err.to_string())),
                    ])]),
                ),
            ]),
        };
        println!("{}", payload.render());
    }
    result.map(|_| ())
}

fn run_tests_command(args: TestArgs, verbose: bool, quiet: bool) -> Result<TestOutcome> {
    let root = project_root(args.manifest_path.as_deref())?;
    let manifest = Manifest::load(&root)?;
    let active_features = manifest.resolve_features(&args.features, args.no_default_features);
    let target = effective_target(args.target.as_deref(), &manifest);
    let sources = test::collect_test_sources(&root)?;
    let build_args = BuildArgs {
        release: false,
        output: None,
        jobs: args.jobs,
        manifest_path: Some(root.clone()),
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        trace: false,
        target: args.target.clone(),
        from: None,
        ignore_warnings: false,
    };
    let link_archives = package_archives_for_runner(&root, &manifest, &build_args, verbose, quiet)?;

    if !quiet {
        println!(
            "\x1b[1;96m     Test\x1b[0m discovered {} source{}",
            sources.len(),
            if sources.len() == 1 { "" } else { "s" }
        );
    }

    let binary = test::build_tests(
        &root,
        &manifest,
        &sources,
        test::TestBuildOptions {
            active_features: &active_features,
            target: target.as_deref(),
            link_archives: &link_archives,
            release: false,
            verbose,
            quiet,
            jobs: resolve_jobs(args.jobs),
        },
    )?;
    let (passed, failed, output) = test::run_tests(&binary, args.filter.as_deref())?;

    if !quiet && !output.trim().is_empty() {
        print!("{}", output);
    }
    if failed > 0 {
        return Err(CbldError::CommandFailed {
            program: binary.display().to_string(),
            code: Some(1),
            stderr: output,
        });
    }

    if !quiet {
        println!(
            "\x1b[1;32m    Finished\x1b[0m test suite: {} passed",
            passed
        );
    }
    Ok(TestOutcome {
        passed,
        failed,
        binary,
    })
}

fn package_archives_for_runner(
    root: &Path,
    manifest: &Manifest,
    build_args: &BuildArgs,
    verbose: bool,
    quiet: bool,
) -> Result<Vec<PathBuf>> {
    let mut archives = Vec::new();
    let Some(package) = manifest.package.as_ref() else {
        return Ok(archives);
    };
    let scan = scan_config(None, package)?;
    if !root.join(&scan.source_dir).is_dir() {
        return Ok(archives);
    }

    if let Ok(layout) = Layout::discover(root, &scan) {
        if matches!(layout.crate_kind, Crate::Library | Crate::Shared) {
            let built = cmd_build(build_args.clone(), verbose, quiet, false)?;
            archives.push(built.artifact);
        }
    }
    Ok(archives)
}

struct BenchOutcome {
    results: Vec<bench::BenchmarkResult>,
    binary: PathBuf,
}

fn cmd_bench(args: BenchArgs, verbose: bool, quiet: bool, json: bool) -> Result<()> {
    let started = Instant::now();
    let result = run_bench_command(args, verbose, if json { true } else { quiet });
    if json {
        let duration_ms = started.elapsed().as_millis() as i64;
        let payload = match &result {
            Ok(outcome) => Json::Object(vec![
                ("status".to_string(), Json::str("success")),
                ("duration_ms".to_string(), Json::Number(duration_ms)),
                (
                    "binary".to_string(),
                    Json::str(outcome.binary.display().to_string()),
                ),
                (
                    "benchmarks".to_string(),
                    Json::Array(
                        outcome
                            .results
                            .iter()
                            .map(|row| {
                                Json::Object(vec![
                                    ("name".to_string(), Json::str(row.name.clone())),
                                    ("real_time_ns".to_string(), Json::Number(row.real_time_ns)),
                                    ("cpu_time_ns".to_string(), Json::Number(row.cpu_time_ns)),
                                    ("iterations".to_string(), Json::Number(row.iterations)),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("errors".to_string(), Json::Array(Vec::new())),
            ]),
            Err(err) => Json::Object(vec![
                ("status".to_string(), Json::str("failure")),
                ("duration_ms".to_string(), Json::Number(duration_ms)),
                (
                    "errors".to_string(),
                    Json::Array(vec![Json::Object(vec![
                        ("severity".to_string(), Json::str("error")),
                        ("message".to_string(), Json::str(err.to_string())),
                    ])]),
                ),
            ]),
        };
        println!("{}", payload.render());
    }
    result.map(|_| ())
}

fn run_bench_command(args: BenchArgs, verbose: bool, quiet: bool) -> Result<BenchOutcome> {
    let root = project_root(args.manifest_path.as_deref())?;
    let manifest = Manifest::load(&root)?;
    let active_features = manifest.resolve_features(&args.features, args.no_default_features);
    let target = effective_target(args.target.as_deref(), &manifest);
    let sources = bench::collect_benchmark_sources(&root)?;
    let build_args = BuildArgs {
        release: args.release,
        output: None,
        jobs: args.jobs,
        manifest_path: Some(root.clone()),
        features: args.features.clone(),
        no_default_features: args.no_default_features,
        trace: false,
        target: args.target.clone(),
        from: None,
        ignore_warnings: false,
    };
    let link_archives = package_archives_for_runner(&root, &manifest, &build_args, verbose, quiet)?;

    if !quiet {
        println!(
            "\x1b[1;95m    Bench\x1b[0m discovered {} source{}",
            sources.len(),
            if sources.len() == 1 { "" } else { "s" }
        );
    }

    let binary = bench::build_benchmarks(
        &root,
        &manifest,
        &sources,
        test::TestBuildOptions {
            active_features: &active_features,
            target: target.as_deref(),
            link_archives: &link_archives,
            release: args.release,
            verbose,
            quiet,
            jobs: resolve_jobs(args.jobs),
        },
    )?;
    let run = bench::run_benchmarks(&binary, args.filter.as_deref())?;

    if !quiet && !run.output.trim().is_empty() {
        print!("{}", run.output);
    }
    if !quiet {
        println!(
            "\x1b[1;32m    Finished\x1b[0m benchmark suite: {} row{}",
            run.results.len(),
            if run.results.len() == 1 { "" } else { "s" }
        );
    }
    Ok(BenchOutcome {
        results: run.results,
        binary,
    })
}

/// Run the (intentionally bare) build, and only pay for environment
/// diagnostics if it actually failed. A successful build never spawns a
/// single extra process beyond what compiling/linking already required —
/// that's what keeps the hot path at `cbld build`'s target of a near-instant
/// invocation. Shared by `cbld build` and `cbld run`, since the latter is
/// just a build with an extra step.
fn build_with_diagnostics(
    args: BuildArgs,
    verbose: bool,
    quiet: bool,
    json: bool,
) -> Result<BuildOutcome> {
    match cmd_build(args, verbose, quiet, json) {
        Ok(outcome) => Ok(outcome),
        Err(err) => {
            if !quiet && !json {
                eprintln!(
                    "\n\x1b[1;33mnote\x1b[0m: build failed — running `cbld doctor` diagnostics...\n"
                );
                let _ = doctor::run(verbose, false);
                eprintln!();
            }
            Err(err)
        }
    }
}

/// `cbld sync` — refresh `~/.cbld/cbld-libs` from the registry.
///
/// Strictly an index refresh: no manifest is loaded, no dependency is
/// resolved, and `cbld.lock` is never touched. Use `cbld update` to
/// re-resolve a project's dependencies instead.
fn cmd_sync(verbose: bool, quiet: bool, json: bool) -> Result<()> {
    let res = (|| {
        let resolver = Resolver::new(verbose)?;
        resolver.sync_index(quiet)
    })();
    if json {
        if let Err(ref e) = res {
            println!(
                "{}",
                Json::Object(vec![
                    ("status".to_string(), Json::str("failure")),
                    ("message".to_string(), Json::str(e.to_string())),
                ])
                .render()
            );
        } else {
            println!(
                "{}",
                Json::Object(vec![("status".to_string(), Json::str("success"))]).render()
            );
        }
    }
    res
}

fn cmd_migrate(args: cli::MigrateArgs, quiet: bool, json: bool) -> Result<()> {
    let res = migrate::run(&args, quiet);
    if json {
        if let Err(ref e) = res {
            println!(
                "{}",
                Json::Object(vec![
                    ("status".to_string(), Json::str("failure")),
                    ("message".to_string(), Json::str(e.to_string())),
                ])
                .render()
            );
        } else {
            println!(
                "{}",
                Json::Object(vec![("status".to_string(), Json::str("success"))]).render()
            );
        }
    }
    res
}

/// `cbld build`
fn cmd_build(args: BuildArgs, verbose: bool, quiet: bool, json: bool) -> Result<BuildOutcome> {
    let root = project_root(args.manifest_path.as_deref())?;
    let manifest = Manifest::load(&root)?;

    let outcome = if manifest.is_workspace() {
        // Workspaces build each member; we surface the last member's artifact.
        build_workspace(&root, &manifest, &args, verbose, quiet, json)?
    } else {
        build_single(&root, &manifest, &args, verbose, quiet, json)?
    };

    // Automatic IDE integration: every successful build regenerates
    // compile_commands.json at the project root, with no flag required.
    if !outcome.compile_commands.is_empty() {
        compdb::write(&root, &outcome.compile_commands)?;
        if verbose {
            eprintln!(
                "  \x1b[2m[cbld]\x1b[0m wrote {} entries to compile_commands.json",
                outcome.compile_commands.len()
            );
        }
    }

    Ok(outcome)
}

/// Build a standalone (non-workspace) package.
fn build_single(
    root: &Path,
    manifest: &Manifest,
    args: &BuildArgs,
    verbose: bool,
    quiet: bool,
    json: bool,
) -> Result<BuildOutcome> {
    // `require_package` runs before layout discovery so `[package] source_dir`
    // (and the CLI `--from` override) can redirect where cbld looks for the
    // entry point — legacy trees whose sources don't live under `src/`.
    let mut package = require_package(manifest, root)?;
    let scan = scan_config(args.from.as_deref(), &package)?;
    let layout = Layout::assert_cbld_standard(root, &scan)?;

    // --- Toolchain pin (opt-in; skipped entirely when unset, preserving the
    // hot-path guarantee documented in website/docs/architecture.md) -------
    if let Some(spec) = &package.toolchain {
        ToolchainSpec::parse(spec)?.validate()?;
    }

    // --- Dependency resolution -------------------------------------------
    // A populated third_party/ takes over entirely: no git, no network, no
    // global resolver cache (see `cbld vendor`).
    let resolved = match vendored_dependencies(root, manifest)? {
        Some(vendored) => vendored,
        None => {
            let resolver = Resolver::new(verbose)?;
            let existing_lock = Lockfile::load(root)?;
            let resolved = resolver.resolve_all(manifest, existing_lock.as_ref())?;

            // Keep cbld.lock in sync with the resolved graph (including
            // newly discovered transitives) so vendor/check see the same set.
            if !resolved.is_empty() {
                let lock = build_lockfile(&resolved);
                lock.save(root)?;
                if existing_lock.is_none() && !quiet {
                    println!(
                        "\x1b[1;32m    Locking\x1b[0m {} dependenc{}",
                        resolved.len(),
                        if resolved.len() == 1 { "y" } else { "ies" }
                    );
                }
            }
            resolved
        }
    };

    // --- Cross-compilation target -----------------------------------------
    // CLI `--target` wins over the manifest's `[package] target`; whichever
    // wins here also governs every dependency compiled below, since a
    // dependency built for the host while the root package targets some
    // other triple would fail to link (mismatched architecture/ABI).
    let target = effective_target(args.target.as_deref(), manifest);
    if verbose {
        if let Some(t) = &target {
            eprintln!("  \x1b[2m[cbld]\x1b[0m cross-compiling for target: {t}");
        }
    }

    // Build dependencies first so their archives/headers exist.
    let target_dir = root.join("target");
    let deps = build_dependencies(&resolved, args, verbose, quiet, json, target.as_deref())?;

    // --- Compile the root package ----------------------------------------
    let features = manifest.resolve_features(&args.features, args.no_default_features);
    if verbose && !features.is_empty() {
        eprintln!(
            "  \x1b[2m[cbld]\x1b[0m active features: {}",
            features.join(", ")
        );
    }

    // Package-level `include_dirs` (legacy support) search first, then
    // dependency headers.
    let mut include_dirs = package_include_dirs(root, &package);
    include_dirs.extend(deps.includes);

    let pkg_cflags = pkg_config_cflags(&package.pkg_config, verbose)?;
    let mut pkg_libs = pkg_config_libs(&package.pkg_config, verbose)?;
    pkg_libs.extend(deps.pkg_libs.clone());
    package.libs.extend(pkg_libs);
    let mut c_profile = manifest.profile.c.clone().unwrap_or_default();
    let mut cpp_profile = manifest.profile.cpp.clone().unwrap_or_default();
    c_profile.extra_flags.extend(pkg_cflags.clone());
    cpp_profile.extra_flags.extend(pkg_cflags);

    let compiler = Compiler::new(
        c_profile,
        cpp_profile,
        root,
        include_dirs,
        &features,
        args.release,
        args.trace,
        target,
        package.defines.clone(),
        args.ignore_warnings || package.ignore_warnings,
    );

    let engine = Engine::new(jobs(args), verbose, quiet, json, args.trace);
    let built = engine.build_package(
        &layout,
        &package,
        &compiler,
        BuildDest {
            target_dir: &target_dir,
            output_name: args.output.as_deref(),
            release: args.release,
        },
        &deps.archives,
    )?;

    let mut compile_commands = deps.compile_commands;
    compile_commands.extend(built.compile_commands);

    Ok(BuildOutcome {
        artifact: built.path,
        crate_kind: layout.crate_kind,
        cache_hits: deps.cache_hits + if built.cache_hit { 1 } else { 0 },
        compile_commands,
    })
}

/// If `<root>/third_party` exists and is non-empty, resolve dependencies
/// entirely from local vendored copies plus `cbld.lock` metadata, with no
/// git or network access at all — the offline/autonomous build path enabled
/// by `cbld vendor`.
fn vendored_dependencies(root: &Path, manifest: &Manifest) -> Result<Option<Vec<ResolvedDep>>> {
    let vendor_dir = root.join("third_party");
    let has_entries = std::fs::read_dir(&vendor_dir)
        .map(|mut d| d.next().is_some())
        .unwrap_or(false);
    if !has_entries {
        return Ok(None);
    }

    let lock = Lockfile::load(root)?.ok_or_else(|| {
        CbldError::Config(
            "third_party/ is populated but cbld.lock is missing; run `cbld vendor` again".into(),
        )
    })?;

    let mut resolved = Vec::with_capacity(lock.dependencies.len());
    for locked in &lock.dependencies {
        let name = &locked.name;
        let cache_path = vendor_dir.join(name);
        if !cache_path.is_dir() {
            return Err(CbldError::Config(format!(
                "third_party/{name} is missing; run `cbld vendor` again"
            )));
        }
        resolved.push(ResolvedDep {
            name: name.clone(),
            shorthand: name.clone(),
            url: locked.source.trim_start_matches("git+").to_string(),
            source: locked.source.clone(),
            version: locked.version.clone(),
            checksum: locked.checksum.clone(),
            cache_path,
            dependencies: locked.dependencies.clone(),
            features: Vec::new(),
        });
    }

    // Recover `{ features = [...] }` requests from the root manifest and
    // each vendored package's own cbld.toml (the lockfile does not store them).
    let mut feature_map: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    merge_requested_features(&mut feature_map, &manifest.dependencies);
    for r in &resolved {
        if let Ok(m) = PackageIndex::load().and_then(|idx| {
            idx.effective_manifest(&r.cache_path, &r.shorthand, &r.url, &r.name, &r.version)
        }) {
            merge_requested_features(&mut feature_map, &m.dependencies);
        }
    }
    for r in &mut resolved {
        if let Some(f) = feature_map.remove(&r.name) {
            r.features = f;
        }
    }

    // Direct deps named in the root manifest must still appear in the lock.
    for shorthand in manifest.dependencies.keys() {
        let name = package_name(shorthand);
        if !resolved.iter().any(|r| r.name == name) {
            return Err(CbldError::Config(format!(
                "vendored dependency '{name}' has no entry in cbld.lock"
            )));
        }
    }
    Ok(Some(resolved))
}

fn merge_requested_features(
    into: &mut std::collections::BTreeMap<String, Vec<String>>,
    deps: &std::collections::BTreeMap<String, Dependency>,
) {
    for (shorthand, dep) in deps {
        let slot = into.entry(package_name(shorthand)).or_default();
        for f in &dep.features {
            if !slot.contains(f) {
                slot.push(f.clone());
            }
        }
    }
}

/// Build every member of a workspace in declaration order.
fn build_workspace(
    root: &Path,
    manifest: &Manifest,
    args: &BuildArgs,
    verbose: bool,
    quiet: bool,
    json: bool,
) -> Result<BuildOutcome> {
    let members = manifest
        .workspace
        .as_ref()
        .map(|w| w.members.clone())
        .unwrap_or_default();

    let mut last: Option<BuildOutcome> = None;
    let mut all_compile_commands = Vec::new();
    for member in &members {
        let member_root = root.join(member);
        let member_manifest = Manifest::load(&member_root)?;
        if !quiet {
            println!("\x1b[1;36m   Workspace\x1b[0m building member '{member}'");
        }
        let outcome = build_single(&member_root, &member_manifest, args, verbose, quiet, json)?;
        all_compile_commands.extend(outcome.compile_commands.clone());
        last = Some(outcome);
    }

    let mut outcome =
        last.ok_or_else(|| CbldError::LayoutViolation("workspace has no members to build".into()))?;
    outcome.compile_commands = all_compile_commands;
    Ok(outcome)
}

/// Build all resolved dependencies and collect their include directories,
/// static archives (in reverse topological order for the linker), how many
/// of them were served from the global build cache, and their
/// compile_commands.json entries.
///
/// `cross_target` is the *root* package's already-resolved effective target
/// triple (CLI `--target` or its manifest's `[package] target`), not each
/// dependency's own manifest field — every dependency is force-compiled for
/// the same triple as the root package, since linking object files built for
/// different targets into one artifact doesn't work.
fn build_dependencies(
    resolved: &[ResolvedDep],
    args: &BuildArgs,
    verbose: bool,
    quiet: bool,
    json: bool,
    cross_target: Option<&str>,
) -> Result<DepArtifacts> {
    let mut includes = Vec::new();
    let mut archives = Vec::new();
    let mut pkg_libs = Vec::new();
    let mut cache_hits = 0usize;
    let mut compile_commands = Vec::new();
    let index = PackageIndex::load()?;

    for dep in resolved {
        // Overlay recipe if the clone has no cbld.toml; a cbld.toml in the
        // clone always wins.
        let dep_manifest = index.effective_manifest(
            &dep.cache_path,
            &dep.shorthand,
            &dep.url,
            &dep.name,
            &dep.version,
        )?;
        let dep_package = require_package(&dep_manifest, &dep.cache_path)?;
        let dep_scan = scan_config(None, &dep_package)?;
        let dep_layout = Layout::discover(&dep.cache_path, &dep_scan)?;

        includes.extend(dependency_include_dirs(&dep.cache_path, &dep_package));

        if dep_layout.crate_kind == Crate::Header {
            if !quiet {
                println!(
                    "\x1b[1;32m     Header\x1b[0m {} v{} (dependency)",
                    dep.name, dep.version
                );
            }
            continue;
        }

        if !quiet {
            println!(
                "\x1b[1;32m   Compiling\x1b[0m {} v{} (dependency)",
                dep.name, dep.version
            );
        }

        // Dependencies are always built as libraries regardless of their own
        // entry kind hint — we link their archive into the consumer.
        let dep_features = dep_manifest.resolve_features(&dep.features, false);
        // Dependencies are never traced: `--trace` profiles the package
        // being actively worked on, not its (already-stable) dependencies.
        // They *are* cross-compiled for `cross_target`, though — see the
        // doc comment above.
        let dep_pkg_cflags = pkg_config_cflags(&dep_package.pkg_config, verbose)?;
        let dep_pkg_libs = pkg_config_libs(&dep_package.pkg_config, verbose)?;
        pkg_libs.extend(dep_pkg_libs.clone());
        let mut dep_c = dep_manifest.profile.c.clone().unwrap_or_default();
        let mut dep_cpp = dep_manifest.profile.cpp.clone().unwrap_or_default();
        dep_c.extra_flags.extend(dep_pkg_cflags.clone());
        dep_cpp.extra_flags.extend(dep_pkg_cflags);
        let dep_compiler = Compiler::new(
            dep_c,
            dep_cpp,
            &dep.cache_path,
            package_include_dirs(&dep.cache_path, &dep_package),
            &dep_features,
            args.release,
            false,
            cross_target.map(|t| t.to_string()),
            dep_package.defines.clone(),
            dep_package.ignore_warnings,
        );

        let dep_target = dep.cache_path.join("target");
        let engine = Engine::new(jobs(args), verbose, quiet, json, false);

        // Force library output for dependencies even if they expose main.*.
        let lib_layout = dep_layout.as_library();
        let built = engine.build_package(
            &lib_layout,
            &dep_package,
            &dep_compiler,
            BuildDest {
                target_dir: &dep_target,
                output_name: None,
                release: args.release,
            },
            &[],
        )?;
        if built.cache_hit {
            cache_hits += 1;
        }
        compile_commands.extend(built.compile_commands);
        archives.push(built.path);
    }

    // GNU ld / lld are single-pass: the archive that *uses* a symbol must
    // appear before the archive that *defines* it. `resolved` is topological
    // (dependencies first), so reverse before handing the list to the linker.
    archives.reverse();

    Ok(DepArtifacts {
        includes,
        archives,
        pkg_libs,
        cache_hits,
        compile_commands,
    })
}

/// `cbld run`
fn cmd_run(args: RunArgs, verbose: bool, quiet: bool, json: bool) -> Result<()> {
    let quiet_eff = quiet || json;
    let res: Result<()> = (|| {
        let outcome = build_with_diagnostics(args.build, verbose, quiet_eff, json)?;
        if outcome.crate_kind != Crate::Executable {
            return Err(CbldError::LayoutViolation(
                "`cbld run` requires an executable (src/main.cpp or src/main.c)".into(),
            ));
        }

        if !quiet_eff {
            println!(
                "\x1b[1;32m     Running\x1b[0m {}",
                outcome.artifact.display()
            );
        }

        let status = Command::new(&outcome.artifact)
            .args(&args.bin_args)
            .status()
            .map_err(|source| CbldError::CommandSpawn {
                program: outcome.artifact.display().to_string(),
                source,
            })?;

        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    })();
    if json {
        if let Err(ref e) = res {
            println!(
                "{}",
                Json::Object(vec![
                    ("status".to_string(), Json::str("failure")),
                    ("message".to_string(), Json::str(e.to_string())),
                ])
                .render()
            );
        } else {
            println!(
                "{}",
                Json::Object(vec![("status".to_string(), Json::str("success"))]).render()
            );
        }
    }
    res
}

/// `cbld check` — run Clang's static analyzer over the package's own
/// sources, with no object files, no linking, and no artifact produced.
///
/// Dependencies are resolved (respecting `third_party/` vendoring, same as
/// `cbld build`) purely to expose their headers on the include path — they
/// are never compiled or analyzed themselves, since `cbld check` audits the
/// package you're working on, not its already-vetted dependencies.
fn cmd_check(args: CheckArgs, verbose: bool, quiet: bool, json: bool) -> Result<()> {
    let quiet_eff = quiet || json;
    let res = (|| -> Result<()> {
        let root = project_root(args.manifest_path.as_deref())?;
        let manifest = Manifest::load(&root)?;
        for_each_package(&root, &manifest, |pkg_root, pkg_manifest| {
            let (layout, compiler) = analysis_compiler(
                pkg_root,
                pkg_manifest,
                &args.features,
                args.no_default_features,
                args.target.as_deref(),
                verbose,
            )?;
            let engine = Engine::new(resolve_jobs(args.jobs), verbose, quiet_eff, json, false);
            engine.check_package(&layout, &compiler)
        })
    })();
    if json {
        if let Err(ref e) = res {
            println!(
                "{}",
                Json::Object(vec![
                    ("status".to_string(), Json::str("failure")),
                    ("message".to_string(), Json::str(e.to_string())),
                ])
                .render()
            );
        } else {
            println!(
                "{}",
                Json::Object(vec![("status".to_string(), Json::str("success"))]).render()
            );
        }
    }
    res
}

/// `cbld fmt` — clang-format the package (or every workspace member).
fn cmd_fmt(args: FmtArgs, verbose: bool, quiet: bool, json: bool) -> Result<()> {
    let quiet_eff = quiet || json;
    let res = (|| -> Result<()> {
        let root = project_root(args.manifest_path.as_deref())?;
        let manifest = Manifest::load(&root)?;
        for_each_package(&root, &manifest, |pkg_root, pkg_manifest| {
            let package = require_package(pkg_manifest, pkg_root)?;
            let scan = scan_config(None, &package)?;
            let layout = Layout::assert_cbld_standard(pkg_root, &scan)?;
            fmt::run(&layout, args.check, verbose, quiet_eff)
        })
    })();
    if json {
        if let Err(ref e) = res {
            println!(
                "{}",
                Json::Object(vec![
                    ("status".to_string(), Json::str("failure")),
                    ("message".to_string(), Json::str(e.to_string())),
                ])
                .render()
            );
        } else {
            println!(
                "{}",
                Json::Object(vec![("status".to_string(), Json::str("success"))]).render()
            );
        }
    }
    res
}

/// `cbld lint` — clang-tidy the package (or every workspace member).
fn cmd_lint(args: LintArgs, verbose: bool, quiet: bool, json: bool) -> Result<()> {
    let quiet_eff = quiet || json;
    let res = (|| -> Result<()> {
        let root = project_root(args.manifest_path.as_deref())?;
        let manifest = Manifest::load(&root)?;
        for_each_package(&root, &manifest, |pkg_root, pkg_manifest| {
            let (layout, compiler) = analysis_compiler(
                pkg_root,
                pkg_manifest,
                &args.features,
                args.no_default_features,
                args.target.as_deref(),
                verbose,
            )?;
            lint::run(&layout, &compiler, args.deny_warnings, verbose, quiet_eff)
        })
    })();
    if json {
        if let Err(ref e) = res {
            println!(
                "{}",
                Json::Object(vec![
                    ("status".to_string(), Json::str("failure")),
                    ("message".to_string(), Json::str(e.to_string())),
                ])
                .render()
            );
        } else {
            println!(
                "{}",
                Json::Object(vec![("status".to_string(), Json::str("success"))]).render()
            );
        }
    }
    res
}

/// Run `f` on a standalone package, or on each `[workspace] members` entry.
fn for_each_package(
    root: &Path,
    manifest: &Manifest,
    mut f: impl FnMut(&Path, &Manifest) -> Result<()>,
) -> Result<()> {
    if manifest.is_workspace() {
        let members = manifest
            .workspace
            .as_ref()
            .map(|w| w.members.clone())
            .unwrap_or_default();
        if members.is_empty() {
            return Err(CbldError::LayoutViolation(
                "workspace has no members".into(),
            ));
        }
        for member in members {
            let member_root = root.join(&member);
            let member_manifest = Manifest::load(&member_root)?;
            f(&member_root, &member_manifest)?;
        }
        Ok(())
    } else {
        f(root, manifest)
    }
}

/// Layout + compiler for commands that parse the package the same way
/// `cbld check` does: resolve deps only for include paths, never compile them.
fn analysis_compiler(
    root: &Path,
    manifest: &Manifest,
    features: &[String],
    no_default_features: bool,
    target: Option<&str>,
    verbose: bool,
) -> Result<(Layout, Compiler)> {
    let package = require_package(manifest, root)?;
    let scan = scan_config(None, &package)?;
    let layout = Layout::assert_cbld_standard(root, &scan)?;

    let resolved = match vendored_dependencies(root, manifest)? {
        Some(vendored) => vendored,
        None => {
            let resolver = Resolver::new(verbose)?;
            let existing_lock = Lockfile::load(root)?;
            resolver.resolve_all(manifest, existing_lock.as_ref())?
        }
    };
    let index = PackageIndex::load()?;
    let mut include_dirs = package_include_dirs(root, &package);
    let mut dep_pkg_cflags: Vec<String> = Vec::new();
    for dep in &resolved {
        let dep_manifest = index.effective_manifest(
            &dep.cache_path,
            &dep.shorthand,
            &dep.url,
            &dep.name,
            &dep.version,
        )?;
        if let Some(pkg) = dep_manifest.package {
            include_dirs.extend(dependency_include_dirs(&dep.cache_path, &pkg));
            if !pkg.pkg_config.is_empty() {
                dep_pkg_cflags.extend(pkg_config_cflags(&pkg.pkg_config, verbose)?);
            }
        }
    }

    let features = manifest.resolve_features(features, no_default_features);
    let target = effective_target(target, manifest);
    let mut c_profile = manifest.profile.c.clone().unwrap_or_default();
    let mut cpp_profile = manifest.profile.cpp.clone().unwrap_or_default();
    let root_cflags = pkg_config_cflags(&package.pkg_config, verbose)?;
    c_profile.extra_flags.extend(root_cflags.clone());
    c_profile.extra_flags.extend(dep_pkg_cflags.clone());
    cpp_profile.extra_flags.extend(root_cflags);
    cpp_profile.extra_flags.extend(dep_pkg_cflags);
    let compiler = Compiler::new(
        c_profile,
        cpp_profile,
        root,
        include_dirs,
        &features,
        false,
        false,
        target,
        package.defines.clone(),
        package.ignore_warnings,
    );
    Ok((layout, compiler))
}

/// `cbld update` — re-resolve from scratch and rewrite the lockfile.
///
/// Strictly a dependency-graph refresh: it never fetches or rewrites the
/// package index (`~/.cbld/cbld-libs`). Use `cbld sync` for that.
fn cmd_update(args: UpdateArgs, verbose: bool, quiet: bool, json: bool) -> Result<()> {
    let quiet_eff = quiet || json;
    let res = (|| -> Result<()> {
        let root = project_root(args.manifest_path.as_deref())?;
        let manifest = Manifest::load(&root)?;
        let resolver = Resolver::new(verbose)?;

        // Pass `None` so resolution fetches fresh HEAD SHAs rather than honoring
        // the existing lock. If a single package was named, keep the others pinned.
        let existing = Lockfile::load(&root)?;
        let pin = match &args.package {
            Some(_) => existing.as_ref(),
            None => None,
        };

        let mut resolved = resolver.resolve_all(&manifest, pin)?;

        // If a specific package was requested, re-resolve only that node
        // (fresh SHA) while the rest stay at their locked SHAs.
        if let Some(only) = &args.package {
            let target = package_name(only);
            let pos = resolved.iter().position(|d| d.name == target).ok_or_else(|| {
                CbldError::Resolution(format!(
                    "'{only}' is not a dependency of this package (use the package name, e.g. the last segment of gh:owner/repo)"
                ))
            })?;
            let existing = resolved[pos].clone();
            let spec = Dependency {
                version: existing.version.clone(),
                features: existing.features.clone(),
                tag: None,
            };
            resolved[pos] = resolver.resolve_one(&existing.shorthand, &spec, None)?;
        }

        let lock = build_lockfile(&resolved);
        lock.save(&root)?;

        if !quiet_eff {
            println!(
                "\x1b[1;32m     Updated\x1b[0m {} dependenc{} in cbld.lock",
                resolved.len(),
                if resolved.len() == 1 { "y" } else { "ies" }
            );
            for dep in &resolved {
                println!(
                    "            {} v{} @ {}",
                    dep.name,
                    dep.version,
                    short_sha(&dep.checksum)
                );
            }
        }
        Ok(())
    })();
    if json {
        if let Err(ref e) = res {
            println!(
                "{}",
                Json::Object(vec![
                    ("status".to_string(), Json::str("failure")),
                    ("message".to_string(), Json::str(e.to_string())),
                ])
                .render()
            );
        } else {
            println!(
                "{}",
                Json::Object(vec![("status".to_string(), Json::str("success"))]).render()
            );
        }
    }
    res
}

/// `cbld vendor` — copy every dependency in `cbld.lock` into a local
/// `third_party/` tree for complete offline autonomy.
///
/// Resolution reuses `Resolver::resolve_all` pinned to the existing lock (the
/// same reproducible path `cbld build` takes), so vendoring never silently
/// re-resolves a dependency to a different commit than what's locked — it
/// only relocates already-resolved sources from the global cache into the
/// project itself.
fn cmd_vendor(args: VendorArgs, verbose: bool, quiet: bool, json: bool) -> Result<()> {
    let quiet_eff = quiet || json;
    let res = (|| -> Result<()> {
        let root = project_root(args.manifest_path.as_deref())?;
        let manifest = Manifest::load(&root)?;
        let lock = Lockfile::load(&root)?.ok_or_else(|| {
            CbldError::Config("no cbld.lock found; run `cbld build` or `cbld update` first".into())
        })?;

        let resolver = Resolver::new(verbose)?;
        let resolved = resolver.resolve_all(&manifest, Some(&lock))?;

        let vendor_dir = root.join("third_party");
        std::fs::create_dir_all(&vendor_dir).path_ctx(&vendor_dir)?;

        for dep in &resolved {
            let dest = vendor_dir.join(&dep.name);
            if dest.exists() {
                std::fs::remove_dir_all(&dest).path_ctx(&dest)?;
            }
            copy_tree_excluding_git(&dep.cache_path, &dest)?;
            if !quiet_eff {
                println!(
                    "\x1b[1;32m    Vendored\x1b[0m {} v{} -> {}",
                    dep.name,
                    dep.version,
                    dest.display()
                );
            }
        }

        if !quiet_eff {
            println!(
                "\x1b[1;32m   Finished\x1b[0m vendoring {} dependenc{} into {}",
                resolved.len(),
                if resolved.len() == 1 { "y" } else { "ies" },
                vendor_dir.display()
            );
        }
        Ok(())
    })();
    if json {
        if let Err(ref e) = res {
            println!(
                "{}",
                Json::Object(vec![
                    ("status".to_string(), Json::str("failure")),
                    ("message".to_string(), Json::str(e.to_string())),
                ])
                .render()
            );
        } else {
            println!(
                "{}",
                Json::Object(vec![("status".to_string(), Json::str("success"))]).render()
            );
        }
    }
    res
}

/// Recursively copy a directory tree, skipping any `.git` directory — the
/// vendored copy is a source snapshot, not a git checkout.
fn copy_tree_excluding_git(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).path_ctx(dst)?;
    for entry in std::fs::read_dir(src).path_ctx(src)? {
        let entry = entry.path_ctx(src)?;
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let dest_path = dst.join(&name);
        if path.is_dir() {
            copy_tree_excluding_git(&path, &dest_path)?;
        } else {
            std::fs::copy(&path, &dest_path).path_ctx(&dest_path)?;
        }
    }
    Ok(())
}

/// `cbld init` — scaffold a new cbld-standard package.
fn cmd_init(args: InitArgs, quiet: bool, json: bool) -> Result<()> {
    let quiet_eff = quiet || json;
    let res = (|| -> Result<()> {
        let root = &args.path;
        std::fs::create_dir_all(root).path_ctx(root)?;

        let name = match &args.name {
            Some(n) => n.clone(),
            None => root
                .canonicalize()
                .ok()
                .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
                .unwrap_or_else(|| "my_project".to_string()),
        };

        if let Some(template) = args.template.as_deref() {
            let tmpl = template.trim();
            if tmpl.is_empty() {
                return Err(CbldError::Config("empty --template value".into()));
            }
            // Template path must be empty or not exist as cbld package.
            if root.join("cbld.toml").exists() {
                return Err(CbldError::LayoutViolation(format!(
                    "{} already contains cbld.toml; refusing to overwrite with template",
                    root.display()
                )));
            }
            // Check if root is non-empty (besides .gitignore).
            if let Ok(entries) = std::fs::read_dir(root) {
                let non_empty = entries.filter_map(|e| e.ok()).any(|e| {
                    let n = e.file_name();
                    n != ".git" && n != ".gitignore"
                });
                if non_empty {
                    return Err(CbldError::LayoutViolation(format!(
                        "{} is not empty; --template requires an empty directory",
                        root.display()
                    )));
                }
            }

            // Local directory template?
            if let Ok(meta) = std::fs::metadata(tmpl) {
                if meta.is_dir() {
                    copy_tree_excluding_git(Path::new(tmpl), root)?;
                } else {
                    return Err(CbldError::Config(format!(
                        "template path is not a directory: {tmpl}"
                    )));
                }
            } else {
                let url = resolve_template_url(tmpl);
                // Remote git template: clone to temp then copy.
                let tmp = std::env::temp_dir().join(format!("cbld-tmpl-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&tmp);
                let mut cmd = Command::new("git");
                cmd.args([
                    "clone",
                    "--depth",
                    "1",
                    &url,
                    tmp.to_string_lossy().as_ref(),
                ]);
                let out = cmd.output().map_err(|e| CbldError::CommandSpawn {
                    program: "git".into(),
                    source: e,
                })?;
                if !out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    return Err(CbldError::CommandFailed {
                        program: "git".into(),
                        code: out.status.code(),
                        stderr: if stderr.is_empty() {
                            format!("git clone {url} failed")
                        } else {
                            stderr
                        },
                    });
                }
                copy_tree_excluding_git(&tmp, root)?;
                let _ = std::fs::remove_dir_all(&tmp);
            }

            // Update package name in the template's cbld.toml if present.
            let manifest_path = root.join("cbld.toml");
            if manifest_path.is_file() {
                if let Ok(text) = std::fs::read_to_string(&manifest_path) {
                    if let Ok(mut manifest) = toml::from_str::<Manifest>(&text) {
                        if let Some(pkg) = manifest.package.as_mut() {
                            pkg.name = name.clone();
                        }
                        if let Ok(new_text) = toml::to_string_pretty(&manifest) {
                            let _ = std::fs::write(&manifest_path, new_text);
                        }
                    }
                }
            }

            // Ensure .gitignore exists.
            let gitignore = root.join(".gitignore");
            if !gitignore.exists() {
                std::fs::write(&gitignore, "/target\n").path_ctx(&gitignore)?;
            }

            if !quiet_eff {
                println!(
                    "\x1b[1;32m     Created\x1b[0m package '{name}' at {} from template {tmpl}",
                    root.display()
                );
            }
            return Ok(());
        }

        let src = root.join("src");
        std::fs::create_dir_all(&src).path_ctx(&src)?;

        let is_lib = args.lib && !args.bin;
        let is_c = args.c;

        // Pick entry file name + language.
        let (entry_file, entry_body) = match (is_lib, is_c) {
            (false, false) => ("main.cpp", CPP_MAIN),
            (false, true) => ("main.c", C_MAIN),
            (true, false) => ("lib.cpp", CPP_LIB),
            (true, true) => ("lib.c", C_LIB),
        };

        let entry_path = src.join(entry_file);
        if entry_path.exists() {
            return Err(CbldError::LayoutViolation(format!(
                "{} already exists; refusing to overwrite",
                entry_path.display()
            )));
        }
        std::fs::write(&entry_path, entry_body).path_ctx(&entry_path)?;

        // Write the manifest.
        let manifest_path = root.join("cbld.toml");
        if !manifest_path.exists() {
            let profile = if is_c { C_PROFILE } else { CPP_PROFILE };
            let manifest = format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n\
                 [features]\ndefault = []\n\n{profile}\n[dependencies]\n"
            );
            std::fs::write(&manifest_path, manifest).path_ctx(&manifest_path)?;
        }

        // A minimal .gitignore so target/ doesn't get committed.
        let gitignore = root.join(".gitignore");
        if !gitignore.exists() {
            std::fs::write(&gitignore, "/target\n").path_ctx(&gitignore)?;
        }

        if !quiet_eff {
            let kind = if is_lib { "library" } else { "executable" };
            let lang = if is_c { "C" } else { "C++" };
            println!(
                "\x1b[1;32m     Created\x1b[0m {lang} {kind} package '{name}' at {}",
                root.display()
            );
        }
        Ok(())
    })();
    if json {
        if let Err(ref e) = res {
            println!(
                "{}",
                Json::Object(vec![
                    ("status".to_string(), Json::str("failure")),
                    ("message".to_string(), Json::str(e.to_string())),
                ])
                .render()
            );
        } else {
            println!(
                "{}",
                Json::Object(vec![("status".to_string(), Json::str("success"))]).render()
            );
        }
    }
    res
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Resolve the project root from an optional `--manifest-path` (which may point
/// at a directory or a `cbld.toml`) or fall back to the current directory.
fn project_root(explicit: Option<&Path>) -> Result<PathBuf> {
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let root = if path.is_file() {
        path.parent().map(|p| p.to_path_buf()).unwrap_or(path)
    } else {
        path
    };
    if !root.join("cbld.toml").is_file() {
        return Err(CbldError::LayoutViolation(format!(
            "no cbld.toml found in {} (run `cbld init` to create a package)",
            root.display()
        )));
    }
    Ok(root)
}

/// Effective job count for an invocation.
fn jobs(args: &BuildArgs) -> usize {
    resolve_jobs(args.jobs)
}

/// Shared by `cbld build`/`cbld run` (via `jobs`) and `cbld check`: an
/// explicit `-j` is floored to a minimum of `1`; an absent one falls back to
/// `default_jobs()` (`std::thread::available_parallelism()`).
fn resolve_jobs(explicit: Option<usize>) -> usize {
    explicit.unwrap_or_else(default_jobs).max(1)
}

/// Resolve the cross-compilation target triple for a build: an explicit
/// `--target` on the CLI always wins; otherwise fall back to the package's
/// own `[package] target` manifest field (if any). `None` means "compile
/// natively" — no `--target` flag reaches clang at all.
fn effective_target(cli_target: Option<&str>, manifest: &Manifest) -> Option<String> {
    cli_target
        .map(|t| t.to_string())
        .or_else(|| manifest.package.as_ref().and_then(|p| p.target.clone()))
}

/// Resolve the sources directory to scan (legacy support). Precedence:
/// `cbld build --from <path>` > `[package] source_dir` > `"src"`. The serde
/// default already collapses the last two into `pkg.source_dir`, so this only
/// has to layer the CLI override on top.
fn effective_source_dir(cli_from: Option<&Path>, pkg: &Package) -> String {
    cli_from
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| pkg.source_dir.clone())
}

/// Assemble the source-scan configuration for a package from its manifest and
/// the CLI `--from` override: the effective source dir, the optional
/// `[package] kind` (parsed and validated here so a bad value fails before any
/// filesystem work), and the `include`/`exclude` globs.
fn scan_config(cli_from: Option<&Path>, pkg: &Package) -> Result<ScanConfig> {
    let kind = match &pkg.kind {
        Some(k) => Some(Crate::parse(k)?),
        None => None,
    };
    Ok(ScanConfig {
        source_dir: effective_source_dir(cli_from, pkg),
        kind,
        include: pkg.include.clone(),
        exclude: pkg.exclude.clone(),
    })
}

/// The package's extra `[package] include_dirs`, resolved relative to its
/// root and prepended to whatever include paths the caller already has
/// (dependency headers). Legacy layouts keep public headers outside
/// `include/`; this is how those directories reach clang as `-I<path>`.
fn package_include_dirs(root: &Path, pkg: &Package) -> Vec<PathBuf> {
    pkg.include_dirs.iter().map(|d| root.join(d)).collect()
}

/// Include paths a *consumer* of this package should search: the conventional
/// `src/` and `include/` directories plus any `[package] include_dirs` from
/// the overlay or the clone's own manifest (cJSON's headers live at `.`).
fn dependency_include_dirs(root: &Path, pkg: &Package) -> Vec<PathBuf> {
    let mut dirs = vec![root.join("src"), root.join("include")];
    dirs.extend(package_include_dirs(root, pkg));
    dirs
}

fn short_sha(sha: &str) -> &str {
    if sha.len() >= 10 {
        &sha[..10]
    } else {
        sha
    }
}

fn pkg_config_cflags(names: &[String], verbose: bool) -> Result<Vec<String>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut cmd = Command::new("pkg-config");
    cmd.arg("--cflags");
    for n in names {
        cmd.arg(n);
    }
    if verbose {
        eprintln!(
            "  \x1b[2m[pkg-config]\x1b[0m pkg-config --cflags {}",
            names.join(" ")
        );
    }
    let out = cmd.output().map_err(|e| {
        CbldError::Environment(format!(
            "pkg-config not found on PATH (needed for pkg_config = [{}]): {e}",
            names.join(", ")
        ))
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(CbldError::Config(format!(
            "pkg-config --cflags {} failed: {}",
            names.join(" "),
            if stderr.is_empty() {
                "package not found".to_string()
            } else {
                stderr
            }
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.split_whitespace().map(|s| s.to_string()).collect())
}

fn pkg_config_libs(names: &[String], verbose: bool) -> Result<Vec<String>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let mut cmd = Command::new("pkg-config");
    cmd.arg("--libs");
    for n in names {
        cmd.arg(n);
    }
    if verbose {
        eprintln!(
            "  \x1b[2m[pkg-config]\x1b[0m pkg-config --libs {}",
            names.join(" ")
        );
    }
    let out = cmd.output().map_err(|e| {
        CbldError::Environment(format!(
            "pkg-config not found on PATH (needed for pkg_config = [{}]): {e}",
            names.join(", ")
        ))
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(CbldError::Config(format!(
            "pkg-config --libs {} failed: {}",
            names.join(" "),
            if stderr.is_empty() {
                "package not found".to_string()
            } else {
                stderr
            }
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.split_whitespace().map(|s| s.to_string()).collect())
}

fn resolve_template_url(tmpl: &str) -> String {
    let t = tmpl.trim();
    if t.contains("://") || t.starts_with("git@") || t.ends_with(".git") {
        return t.to_string();
    }
    if let Some(rest) = t.strip_prefix("gh:") {
        let base = rest.trim_end_matches(".git");
        return format!("https://github.com/{base}.git");
    }
    if let Some(rest) = t.strip_prefix("gl:") {
        let base = rest.trim_end_matches(".git");
        return format!("https://gitlab.com/{base}.git");
    }
    if let Some(rest) = t.strip_prefix("cb:") {
        let base = rest.trim_end_matches(".git");
        return format!("https://codeberg.org/{base}.git");
    }
    if let Some(rest) = t.strip_prefix("sh:") {
        let base = rest.trim_end_matches(".git");
        return format!("https://git.sr.ht/~{base}");
    }
    if t.contains('/') && !t.contains(' ') && !Path::new(t).exists() {
        // Bare owner/repo → GitHub
        let base = t.trim_end_matches(".git");
        return format!("https://github.com/{base}.git");
    }
    t.to_string()
}

// --- Scaffolding templates -------------------------------------------------

const CPP_MAIN: &str = "#include <iostream>\n\n\
int main() {\n    \
std::cout << \"Hello from cbld!\" << std::endl;\n    \
return 0;\n}\n";

const C_MAIN: &str = "#include <stdio.h>\n\n\
int main(void) {\n    \
printf(\"Hello from cbld!\\n\");\n    \
return 0;\n}\n";

const CPP_LIB: &str = "// Library entry point.\n\n\
int cbld_add(int a, int b) {\n    \
return a + b;\n}\n";

const C_LIB: &str = "/* Library entry point. */\n\n\
int cbld_add(int a, int b) {\n    \
return a + b;\n}\n";

const CPP_PROFILE: &str = "[profile.cpp]\nstandard = \"c++20\"\nrtti = false\n\
exceptions = true\nwarnings = [\"all\", \"extra\"]\noptimization = \"0\"\n";

const C_PROFILE: &str = "[profile.c]\nstandard = \"c17\"\n\
warnings = [\"all\", \"extra\"]\noptimization = \"0\"\n";

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::{Dependency, LockedDependency};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let n = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("cbld-main-test-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest_with_dependency(shorthand: &str) -> Manifest {
        let mut manifest = Manifest::default();
        manifest.dependencies.insert(
            shorthand.to_string(),
            Dependency {
                version: "1.0".to_string(),
                features: Vec::new(),
                tag: None,
            },
        );
        manifest
    }

    #[test]
    fn vendored_dependencies_is_none_without_a_populated_third_party_dir() {
        let root = temp_dir("no-vendor");
        let manifest = manifest_with_dependency("gh:user/lib");
        assert!(vendored_dependencies(&root, &manifest).unwrap().is_none());

        // An empty third_party/ (created but with nothing in it) also counts
        // as "not vendored" — vendoring is only active once it has content.
        std::fs::create_dir_all(root.join("third_party")).unwrap();
        assert!(vendored_dependencies(&root, &manifest).unwrap().is_none());
    }

    #[test]
    fn vendored_dependencies_errors_when_lock_is_missing() {
        let root = temp_dir("missing-lock");
        std::fs::create_dir_all(root.join("third_party").join("lib")).unwrap();
        let manifest = manifest_with_dependency("gh:user/lib");

        let err = vendored_dependencies(&root, &manifest).unwrap_err();
        assert!(err.to_string().contains("cbld.lock"));
    }

    #[test]
    fn vendored_dependencies_resolves_from_local_copies_and_lock_metadata() {
        let root = temp_dir("vendored");
        let dep_dir = root.join("third_party").join("lib");
        std::fs::create_dir_all(&dep_dir).unwrap();

        let lock = Lockfile {
            dependencies: vec![LockedDependency {
                name: "lib".to_string(),
                source: "git+https://example.com/user/lib.git".to_string(),
                checksum: "deadbeef".to_string(),
                version: "1.0".to_string(),
                dependencies: Vec::new(),
            }],
        };
        lock.save(&root).unwrap();

        let manifest = manifest_with_dependency("gh:user/lib");
        let resolved = vendored_dependencies(&root, &manifest)
            .unwrap()
            .expect("third_party/ is populated, so this must resolve locally");

        assert_eq!(resolved.len(), 1);
        let dep = &resolved[0];
        assert_eq!(dep.name, "lib");
        assert_eq!(dep.checksum, "deadbeef");
        assert_eq!(dep.cache_path, dep_dir);
        assert_eq!(dep.url, "https://example.com/user/lib.git");
    }

    #[test]
    fn vendored_dependencies_errors_when_a_dependency_directory_is_missing() {
        let root = temp_dir("partial-vendor");
        // third_party/ has *something* in it, but not the dependency itself.
        std::fs::create_dir_all(root.join("third_party").join("other")).unwrap();

        let lock = Lockfile {
            dependencies: vec![LockedDependency {
                name: "lib".to_string(),
                source: "git+https://example.com/user/lib.git".to_string(),
                checksum: "deadbeef".to_string(),
                version: "1.0".to_string(),
                dependencies: Vec::new(),
            }],
        };
        lock.save(&root).unwrap();

        let manifest = manifest_with_dependency("gh:user/lib");
        let err = vendored_dependencies(&root, &manifest).unwrap_err();
        assert!(err.to_string().contains("third_party/lib"));
    }

    #[test]
    fn copy_tree_excluding_git_skips_git_dir_and_copies_nested_files() {
        let root = temp_dir("copy-tree");
        let src = root.join("src");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join("top.txt"), "top").unwrap();
        std::fs::write(src.join("nested").join("deep.txt"), "deep").unwrap();
        std::fs::write(src.join(".git").join("HEAD"), "ref: refs/heads/main").unwrap();

        let dst = root.join("dst");
        copy_tree_excluding_git(&src, &dst).unwrap();

        assert_eq!(std::fs::read_to_string(dst.join("top.txt")).unwrap(), "top");
        assert_eq!(
            std::fs::read_to_string(dst.join("nested").join("deep.txt")).unwrap(),
            "deep"
        );
        assert!(!dst.join(".git").exists());
    }

    #[test]
    fn build_success_payload_renders_expected_fields() {
        let outcome = BuildOutcome {
            artifact: PathBuf::from("/tmp/target/debug/app"),
            crate_kind: Crate::Executable,
            cache_hits: 2,
            compile_commands: Vec::new(),
        };
        let rendered = build_success_payload(&outcome, 1234).render();
        assert!(rendered.contains("\"status\":\"success\""));
        assert!(rendered.contains("\"duration_ms\":1234"));
        assert!(rendered.contains("\"cache_hits\":2"));
        assert!(rendered.contains("\"artifact\":\"/tmp/target/debug/app\""));
        assert!(rendered.contains("\"errors\":[]"));
    }

    #[test]
    fn build_failure_payload_surfaces_structured_compile_diagnostics() {
        let err = CbldError::Compilation {
            failures: 1,
            diagnostics: vec![error::CompileDiagnostic {
                file: PathBuf::from("src/main.c"),
                line: 4,
                column: 2,
                severity: "error",
                message: "undeclared identifier 'foo'".to_string(),
            }],
        };
        let rendered = build_failure_payload(&err, 42).render();
        assert!(rendered.contains("\"status\":\"failure\""));
        assert!(rendered.contains("\"duration_ms\":42"));
        assert!(rendered.contains("\"line\":4"));
        assert!(rendered.contains("undeclared identifier"));
    }

    #[test]
    fn build_failure_payload_falls_back_to_display_text_for_non_compilation_errors() {
        let err = CbldError::Config("bad toolchain spec".to_string());
        let rendered = build_failure_payload(&err, 7).render();
        assert!(rendered.contains("\"status\":\"failure\""));
        assert!(rendered.contains("bad toolchain spec"));
    }
}
