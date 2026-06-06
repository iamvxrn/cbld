//! cbld — entry point (v0.1.0: `build` only).

mod cli;
mod compdb;
mod compiler;
mod engine;
mod error;
mod glob;
mod json;
mod manifest;
mod trace;

use std::path::{Path, PathBuf};

use cli::{BuildArgs, Cli, Command as Cmd};
use compiler::Compiler;
use engine::{default_jobs, require_package, Crate, Engine, Layout, ScanConfig};
use error::{CbldError, Result};
use manifest::{Manifest, Package, ToolchainSpec};

fn main() {
    let cli = Cli::parse_args();
    let verbose = cli.verbose > 0;
    let quiet = cli.quiet;

    let result = match cli.command {
        Cmd::Build(args) => cmd_build(args, verbose, quiet),
    };

    if let Err(err) = result {
        eprintln!("\x1b[1;31merror\x1b[0m: {err}");
        let mut source = std::error::Error::source(&err);
        while let Some(cause) = source {
            eprintln!("  \x1b[2mcaused by:\x1b[0m {cause}");
            source = cause.source();
        }
        std::process::exit(1);
    }
}

fn cmd_build(args: BuildArgs, verbose: bool, quiet: bool) -> Result<()> {
    let root = project_root(args.manifest_path.as_deref())?;
    let manifest = Manifest::load(&root)?;
    let package = require_package(&manifest, &root)?;
    let scan = scan_config(None, &package)?;
    let layout = Layout::assert_cbld_standard(&root, &scan)?;

    if let Some(spec) = &package.toolchain {
        ToolchainSpec::parse(spec)?.validate()?;
    }

    let target_dir = root.join("target");
    let compiler = Compiler::new(
        manifest.profile.c.clone().unwrap_or_default(),
        manifest.profile.cpp.clone().unwrap_or_default(),
        &root,
        package_include_dirs(&root, &package),
        &[],
        args.release,
        false,
        None,
        package.defines.clone(),
        false,
    );

    let engine = Engine::new(jobs(&args), verbose, quiet, false, false);
    let built = engine.build_package(
        &layout,
        &package,
        &compiler,
        &target_dir,
        args.output.as_deref(),
        args.release,
    )?;

    if !built.compile_commands.is_empty() {
        compdb::write(&root, &built.compile_commands)?;
    }

    Ok(())
}

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

