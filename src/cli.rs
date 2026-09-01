//! Command-line interface definitions.
//!
//! Everything here is pure data: clap derive structs and enums describing the
//! surface of the `cbld` binary. No logic lives here beyond what clap needs to
//! parse arguments. The runtime engine consumes these structures in `main.rs`.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// cbld — a build system for C and C++.
#[derive(Parser, Debug)]
#[command(
    name = "cbld",
    version,
    about = "A build system for C and C++.",
    long_about = "cbld is a build system for C and C++ with strict \
                  project layout, Clang integration, and reproducible builds.",
    propagate_version = true
)]
pub struct Cli {
    /// Increase output verbosity (-v, -vv).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress non-essential output.
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Emit machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,

    /// The subcommand to execute.
    #[command(subcommand)]
    pub command: Command,
}

/// All top-level subcommands cbld understands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Compile the current package and all of its dependencies.
    Build(BuildArgs),

    /// Build (if needed) and then run the resulting executable.
    Run(RunArgs),

    /// Create a new cbld package in the given directory (or current dir).
    #[command(alias = "new")]
    Init(InitArgs),

    /// Re-resolve project dependencies and rewrite cbld.lock.
    ///
    /// Touches only the current project's dependency graph: cache checkouts
    /// under `~/.cbld/cache` and `cbld.lock`. Never touches the package index
    /// (`~/.cbld/cbld-libs`) — see `Sync` for that.
    Update(UpdateArgs),

    /// Diagnose the local toolchain and environment, refreshing the package index.
    Doctor,

    /// Refresh the local package index (~/.cbld/cbld-libs) from a remote index.
    ///
    /// Uses the official index by default; `CBLD_LIBS_URL` overrides it. Touches
    /// only that one index file via native OS fetch tools (TOML recipes or the
    /// legacy `shorthand <url>` format).
    /// Never resolves dependencies, never touches a project's `cbld.lock` —
    /// see `Update` for that. A handful of overlay recipes ship built-in and
    /// work without syncing.
    Sync,

    /// Generate a starter cbld.toml from an existing build system's config.
    Migrate(MigrateArgs),

    /// Vendor every dependency in cbld.lock into a local third_party/ tree.
    ///
    /// Once third_party/ is populated, subsequent `cbld build` runs resolve
    /// dependencies entirely from those local copies — no git, no network,
    /// no global cache lookups.
    Vendor(VendorArgs),

    /// Run Clang's static analyzer over the package's sources without
    /// compiling to an object file or invoking the linker.
    Check(CheckArgs),

    /// Format C/C++ sources and public headers with clang-format (`cargo fmt`).
    Fmt(FmtArgs),

    /// Lint C/C++ sources and public headers with clang-tidy (`cargo clippy`).
    Lint(LintArgs),

    /// Compile and run tests (like `cargo test`).
    Test(TestArgs),

    /// Compile and run benchmarks (like `cargo bench`).
    Bench(BenchArgs),

    /// Generate shell completion scripts for the specified shell.
    Completions(CompletionsArgs),
}

/// Arguments for `completions`.
#[derive(clap::Args, Debug, Clone)]
pub struct CompletionsArgs {
    /// Target shell for auto-completion generation (bash, zsh, fish, powershell, elvish).
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

/// Arguments shared by `build` and (transitively) `run`.
#[derive(clap::Args, Debug, Clone)]
pub struct BuildArgs {
    /// Build with optimizations (uses the release configuration).
    #[arg(long)]
    pub release: bool,

    /// Override the output artifact name.
    #[arg(short = 'o', long = "output", value_name = "NAME")]
    pub output: Option<String>,

    /// Number of parallel compile jobs. Defaults to the number of CPUs.
    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<usize>,

    /// Path to the project root (defaults to the current directory).
    #[arg(long, value_name = "DIR")]
    pub manifest_path: Option<PathBuf>,

    /// Comma-separated list of features to activate.
    #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
    pub features: Vec<String>,

    /// Do not activate the `default` feature set.
    #[arg(long)]
    pub no_default_features: bool,

    /// Profile compilation with Clang's `-ftime-trace` and aggregate the
    /// result into `target/<profile>/cbld_profile.json` (loadable at
    /// chrome://tracing or speedscope.app), printing the slowest headers
    /// and template instantiations to the terminal.
    #[arg(long)]
    pub trace: bool,

    /// Cross-compile for a different target triple (e.g.
    /// `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`), passed to clang
    /// as `--target=<triple>` during both compilation and linking. Overrides
    /// the `[package] target` manifest field when both are set.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,

    /// Scan this directory for sources instead of the manifest's
    /// `[package] source_dir` (which itself defaults to `src`). Legacy escape
    /// hatch: build a tree whose sources don't live under `src/` without
    /// editing the manifest. Overrides `source_dir` when both are set.
    #[arg(long, value_name = "PATH")]
    pub from: Option<PathBuf>,

    /// Suppress all compiler warnings by injecting `-w`. Turns warnings off
    /// regardless of the `[package] ignore_warnings` manifest field — a blunt
    /// tool for building noisy legacy code you don't own.
    #[arg(long)]
    pub ignore_warnings: bool,
}

/// Arguments for `cbld check`.
///
/// Deliberately a smaller surface than `BuildArgs`: `cbld check` never
/// produces an artifact, so `--release`/`-o`/`--trace` don't apply.
#[derive(clap::Args, Debug)]
pub struct CheckArgs {
    /// Path to the project root (defaults to the current directory).
    #[arg(long, value_name = "DIR")]
    pub manifest_path: Option<PathBuf>,

    /// Number of parallel analysis jobs. Defaults to the number of CPUs.
    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<usize>,

    /// Comma-separated list of features to activate.
    #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
    pub features: Vec<String>,

    /// Do not activate the `default` feature set.
    #[arg(long)]
    pub no_default_features: bool,

    /// Analyze as if cross-compiling for this target triple, passed to
    /// clang as `--target=<triple>`. Overrides the `[package] target`
    /// manifest field when both are set.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,
}

/// Arguments for `cbld fmt`.
#[derive(clap::Args, Debug)]
pub struct FmtArgs {
    /// Don't write files; fail if clang-format would rewrite any of them
    /// (`cargo fmt --check`).
    #[arg(long)]
    pub check: bool,

    /// Path to the project root (defaults to the current directory).
    #[arg(long, value_name = "DIR")]
    pub manifest_path: Option<PathBuf>,
}

/// Arguments for `cbld lint`.
///
/// Same analysis-shaped flags as `cbld check` (features, target, include
/// paths from dependencies) plus `--deny-warnings` for clippy's `-D warnings`.
#[derive(clap::Args, Debug)]
pub struct LintArgs {
    /// Treat every clang-tidy finding as an error (`clippy -D warnings`).
    #[arg(long)]
    pub deny_warnings: bool,

    /// Path to the project root (defaults to the current directory).
    #[arg(long, value_name = "DIR")]
    pub manifest_path: Option<PathBuf>,

    /// Comma-separated list of features to activate.
    #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
    pub features: Vec<String>,

    /// Do not activate the `default` feature set.
    #[arg(long)]
    pub no_default_features: bool,

    /// Parse as if cross-compiling for this target triple. Overrides the
    /// `[package] target` manifest field when both are set.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,
}

/// Arguments for `cbld test`.
#[derive(clap::Args, Debug)]
pub struct TestArgs {
    /// Path to the project root (defaults to the current directory).
    #[arg(long, value_name = "DIR")]
    pub manifest_path: Option<PathBuf>,

    /// Number of parallel compile jobs. Defaults to the number of CPUs.
    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<usize>,

    /// Comma-separated list of features to activate.
    #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
    pub features: Vec<String>,

    /// Do not activate the `default` feature set.
    #[arg(long)]
    pub no_default_features: bool,

    /// Analyze and link as if targeting this triple.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,

    /// Filter test names (passed to the test binary as --gtest_filter or as plain arg).
    #[arg(value_name = "FILTER")]
    pub filter: Option<String>,
}

/// Arguments for `cbld bench`.
#[derive(clap::Args, Debug)]
pub struct BenchArgs {
    /// Path to the project root (defaults to the current directory).
    #[arg(long, value_name = "DIR")]
    pub manifest_path: Option<PathBuf>,

    /// Number of parallel compile jobs. Defaults to the number of CPUs.
    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<usize>,

    /// Build benchmarks with optimizations.
    #[arg(long)]
    pub release: bool,

    /// Comma-separated list of features to activate.
    #[arg(long, value_name = "FEATURES", value_delimiter = ',')]
    pub features: Vec<String>,

    /// Do not activate the `default` feature set.
    #[arg(long)]
    pub no_default_features: bool,

    /// Analyze and link as if targeting this triple.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,

    /// Google Benchmark filter expression.
    #[arg(value_name = "FILTER")]
    pub filter: Option<String>,
}

/// Arguments for `cbld run`.
#[derive(clap::Args, Debug)]
pub struct RunArgs {
    /// Build configuration flags shared with `build`.
    #[command(flatten)]
    pub build: BuildArgs,

    /// Arguments forwarded verbatim to the executed binary (after `--`).
    #[arg(last = true, value_name = "ARGS")]
    pub bin_args: Vec<String>,
}

/// Arguments for `cbld init`.
#[derive(clap::Args, Debug)]
pub struct InitArgs {
    /// Directory to initialize. Created if it does not exist.
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: PathBuf,

    /// Package name (defaults to the directory name).
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Initialize a library (`src/lib.cpp`) instead of an executable.
    #[arg(long, conflicts_with = "bin")]
    pub lib: bool,

    /// Initialize an executable (`src/main.cpp`). This is the default.
    #[arg(long)]
    pub bin: bool,

    /// Generate C sources/profile instead of C++.
    #[arg(long)]
    pub c: bool,

    /// Clone a template repository (gh shorthand, URL, or local path) instead of scaffolding from scratch.
    #[arg(long, value_name = "TEMPLATE")]
    pub template: Option<String>,
}

/// Arguments for `cbld update`.
#[derive(clap::Args, Debug)]
pub struct UpdateArgs {
    /// Path to the project root (defaults to the current directory).
    #[arg(long, value_name = "DIR")]
    pub manifest_path: Option<PathBuf>,

    /// Only update this specific dependency (by package name).
    #[arg(value_name = "PACKAGE")]
    pub package: Option<String>,
}

/// Arguments for `cbld vendor`.
#[derive(clap::Args, Debug)]
pub struct VendorArgs {
    /// Path to the project root (defaults to the current directory).
    #[arg(long, value_name = "DIR")]
    pub manifest_path: Option<PathBuf>,
}

/// Arguments for `cbld migrate`.
#[derive(clap::Args, Debug)]
pub struct MigrateArgs {
    /// The build system to migrate from. Only "cmake" is supported today.
    #[arg(long, default_value = "cmake", value_name = "SYSTEM")]
    pub from: String,

    /// Directory containing the source build system's files (defaults to the
    /// current directory).
    #[arg(long, value_name = "DIR")]
    pub path: Option<PathBuf>,
}

impl Cli {
    /// Parse from the real process arguments.
    pub fn parse_args() -> Self {
        Cli::parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// `sync` and `update` are separate `Command` variants — parsing one
    /// invocation resolves to exactly one of them, never both, mirroring the
    /// fact that they touch entirely disjoint state (the global package
    /// index vs. one project's lockfile).
    #[test]
    fn sync_and_update_resolve_to_distinct_variants() {
        let sync = Cli::try_parse_from(["cbld", "sync"]).expect("sync should parse");
        match sync.command {
            Command::Sync => {}
            other => panic!("expected Command::Sync, got {other:?}"),
        }

        let update = Cli::try_parse_from(["cbld", "update"]).expect("update should parse");
        match update.command {
            Command::Update(_) => {}
            other => panic!("expected Command::Update, got {other:?}"),
        }

        // A single invocation only ever resolves one subcommand; trailing
        // tokens that look like another subcommand are rejected as stray
        // arguments rather than silently accepted.
        assert!(Cli::try_parse_from(["cbld", "sync", "update"]).is_err());
    }

    #[test]
    fn update_accepts_an_optional_package_argument() {
        let cli = Cli::try_parse_from(["cbld", "update", "mylib"]).unwrap();
        match cli.command {
            Command::Update(args) => assert_eq!(args.package, Some("mylib".to_string())),
            other => panic!("expected Command::Update, got {other:?}"),
        }

        let cli = Cli::try_parse_from(["cbld", "update"]).unwrap();
        match cli.command {
            Command::Update(args) => assert_eq!(args.package, None),
            other => panic!("expected Command::Update, got {other:?}"),
        }
    }

    #[test]
    fn sync_takes_no_positional_arguments() {
        assert!(Cli::try_parse_from(["cbld", "sync", "extra"]).is_err());
    }

    /// `--json` is declared `global = true`, so it must parse both before
    /// and after the subcommand, on any command — not just `build`/`doctor`
    /// (those two are the only ones that currently *act* on it).
    #[test]
    fn json_flag_is_global_and_defaults_to_false() {
        let cli = Cli::try_parse_from(["cbld", "build"]).unwrap();
        assert!(!cli.json);

        let before = Cli::try_parse_from(["cbld", "--json", "build"]).unwrap();
        assert!(before.json);

        let after = Cli::try_parse_from(["cbld", "doctor", "--json"]).unwrap();
        assert!(after.json);
    }

    #[test]
    fn build_target_flag_defaults_to_none_and_parses_when_given() {
        let cli = Cli::try_parse_from(["cbld", "build"]).unwrap();
        match cli.command {
            Command::Build(args) => assert_eq!(args.target, None),
            other => panic!("expected Command::Build, got {other:?}"),
        }

        let cli = Cli::try_parse_from(["cbld", "build", "--target", "aarch64-unknown-linux-gnu"])
            .unwrap();
        match cli.command {
            Command::Build(args) => {
                assert_eq!(args.target.as_deref(), Some("aarch64-unknown-linux-gnu"))
            }
            other => panic!("expected Command::Build, got {other:?}"),
        }
    }

    #[test]
    fn build_from_and_ignore_warnings_default_off_and_parse_when_given() {
        let cli = Cli::try_parse_from(["cbld", "build"]).unwrap();
        match cli.command {
            Command::Build(args) => {
                assert_eq!(args.from, None);
                assert!(!args.ignore_warnings);
            }
            other => panic!("expected Command::Build, got {other:?}"),
        }

        let cli =
            Cli::try_parse_from(["cbld", "build", "--from", "legacy/src", "--ignore-warnings"])
                .unwrap();
        match cli.command {
            Command::Build(args) => {
                assert_eq!(
                    args.from.as_deref(),
                    Some(std::path::Path::new("legacy/src"))
                );
                assert!(args.ignore_warnings);
            }
            other => panic!("expected Command::Build, got {other:?}"),
        }
    }

    /// `cbld run` flattens `BuildArgs`, so the legacy build flags must be
    /// reachable through it too.
    #[test]
    fn run_inherits_from_and_ignore_warnings_flags() {
        let cli =
            Cli::try_parse_from(["cbld", "run", "--from", "app", "--ignore-warnings"]).unwrap();
        match cli.command {
            Command::Run(args) => {
                assert_eq!(
                    args.build.from.as_deref(),
                    Some(std::path::Path::new("app"))
                );
                assert!(args.build.ignore_warnings);
            }
            other => panic!("expected Command::Run, got {other:?}"),
        }
    }

    #[test]
    fn check_parses_as_its_own_command_with_build_like_flags() {
        let cli = Cli::try_parse_from(["cbld", "check"]).unwrap();
        match cli.command {
            Command::Check(args) => {
                assert_eq!(args.target, None);
                assert!(!args.no_default_features);
            }
            other => panic!("expected Command::Check, got {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "cbld",
            "check",
            "--target",
            "wasm32-unknown-unknown",
            "--features",
            "a,b",
        ])
        .unwrap();
        match cli.command {
            Command::Check(args) => {
                assert_eq!(args.target.as_deref(), Some("wasm32-unknown-unknown"));
                assert_eq!(args.features, vec!["a".to_string(), "b".to_string()]);
            }
            other => panic!("expected Command::Check, got {other:?}"),
        }
    }

    #[test]
    fn fmt_parses_check_flag() {
        let cli = Cli::try_parse_from(["cbld", "fmt"]).unwrap();
        match cli.command {
            Command::Fmt(args) => {
                assert!(!args.check);
                assert!(args.manifest_path.is_none());
            }
            other => panic!("expected Command::Fmt, got {other:?}"),
        }

        let cli = Cli::try_parse_from(["cbld", "fmt", "--check"]).unwrap();
        match cli.command {
            Command::Fmt(args) => assert!(args.check),
            other => panic!("expected Command::Fmt, got {other:?}"),
        }
    }

    #[test]
    fn lint_parses_deny_warnings_flag() {
        let cli = Cli::try_parse_from(["cbld", "lint"]).unwrap();
        match cli.command {
            Command::Lint(args) => assert!(!args.deny_warnings),
            other => panic!("expected Command::Lint, got {other:?}"),
        }

        let cli = Cli::try_parse_from(["cbld", "lint", "--deny-warnings"]).unwrap();
        match cli.command {
            Command::Lint(args) => assert!(args.deny_warnings),
            other => panic!("expected Command::Lint, got {other:?}"),
        }
    }

    #[test]
    fn test_parses_filter_and_build_flags() {
        let cli = Cli::try_parse_from([
            "cbld",
            "test",
            "Smoke.*",
            "--jobs",
            "2",
            "--features",
            "fast,simd",
        ])
        .unwrap();
        match cli.command {
            Command::Test(args) => {
                assert_eq!(args.filter.as_deref(), Some("Smoke.*"));
                assert_eq!(args.jobs, Some(2));
                assert_eq!(args.features, vec!["fast", "simd"]);
            }
            other => panic!("expected Command::Test, got {other:?}"),
        }
    }

    #[test]
    fn bench_parses_filter_release_and_build_flags() {
        let cli = Cli::try_parse_from([
            "cbld",
            "bench",
            "BM_sort",
            "--release",
            "--jobs",
            "3",
            "--target",
            "x86_64-unknown-linux-gnu",
        ])
        .unwrap();
        match cli.command {
            Command::Bench(args) => {
                assert_eq!(args.filter.as_deref(), Some("BM_sort"));
                assert!(args.release);
                assert_eq!(args.jobs, Some(3));
                assert_eq!(args.target.as_deref(), Some("x86_64-unknown-linux-gnu"));
            }
            other => panic!("expected Command::Bench, got {other:?}"),
        }
    }

    #[test]
    fn vendor_parses_as_its_own_command_with_no_positional_arguments() {
        let cli = Cli::try_parse_from(["cbld", "vendor"]).unwrap();
        match cli.command {
            Command::Vendor(args) => assert!(args.manifest_path.is_none()),
            other => panic!("expected Command::Vendor, got {other:?}"),
        }
        assert!(Cli::try_parse_from(["cbld", "vendor", "extra"]).is_err());
    }

    /// The two variants also carry distinct, doc-comment-derived help text —
    /// the structural separation in `Command` is documented, not just an
    /// implementation detail invisible to `--help`.
    #[test]
    fn sync_and_update_have_distinct_help_text() {
        let cmd = Cli::command();

        let sync_about = cmd
            .find_subcommand("sync")
            .and_then(|c| c.get_about())
            .map(|s| s.to_string())
            .expect("sync subcommand should document itself");
        let update_about = cmd
            .find_subcommand("update")
            .and_then(|c| c.get_about())
            .map(|s| s.to_string())
            .expect("update subcommand should document itself");

        assert_ne!(sync_about, update_about);
        assert!(sync_about.to_lowercase().contains("index"));
        assert!(update_about.to_lowercase().contains("lock"));
    }
}
