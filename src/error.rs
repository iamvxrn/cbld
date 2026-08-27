//! Centralized error handling for cbld.
//!
//! A single concrete error enum keeps things flat and pragmatic. Every
//! fallible operation in cbld returns `Result<T, CbldError>`. We deliberately
//! avoid a trait-object based error hierarchy; a rich sum type is clearer and
//! lets call sites `match` on exactly what went wrong.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// The single error type used throughout cbld.
#[derive(Debug)]
pub enum CbldError {
    /// An underlying I/O failure, annotated with the path it concerned.
    Io {
        path: Option<PathBuf>,
        source: io::Error,
    },

    /// The manifest (`cbld.toml`) could not be parsed.
    ManifestParse { path: PathBuf, message: String },

    /// The lockfile (`cbld.lock`) could not be parsed.
    LockParse { path: PathBuf, message: String },

    /// Serialization back to TOML failed.
    Serialize(String),

    /// A required file or directory in the strict layout was missing.
    LayoutViolation(String),

    /// The repository does not adhere to the cbld standard.
    NotCbldStandard { path: PathBuf, reason: String },

    /// A dependency could not be resolved.
    Resolution(String),

    /// An external command (`git`, `curl`, `clang`) failed to even start.
    CommandSpawn { program: String, source: io::Error },

    /// An external command ran but exited non-zero.
    CommandFailed {
        program: String,
        code: Option<i32>,
        stderr: String,
    },

    /// Compilation failed; carries the count of failed translation units and
    /// the structured diagnostics behind them (consumed by `--json` output,
    /// in addition to the terminal renderer in `engine.rs`).
    Compilation {
        failures: usize,
        diagnostics: Vec<CompileDiagnostic>,
    },

    /// `cbld check` failed: at least one source file couldn't even be parsed
    /// by Clang's analyzer. Diagnostics themselves are already streamed to
    /// the terminal as each unit finishes (`Engine::check_package`), so —
    /// unlike `Compilation`, whose structured diagnostics also feed `cbld
    /// build --json` — this only needs the count. Kept as its own variant
    /// so the top-line message doesn't claim a "build" happened when `cbld
    /// check` never compiles or links anything.
    Analysis { failures: usize },

    /// `cbld fmt --check` would rewrite at least one file.
    FmtCheck,

    /// `cbld lint` failed: clang-tidy reported findings (with
    /// `--deny-warnings`) or could not parse a file.
    Lint,

    /// A configuration value was invalid (e.g. unknown optimization level).
    Config(String),

    /// The user's environment is missing something required (e.g. HOME).
    Environment(String),
}

impl fmt::Display for CbldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CbldError::Io { path, source } => match path {
                Some(p) => write!(f, "I/O error at '{}': {}", p.display(), source),
                None => write!(f, "I/O error: {}", source),
            },
            CbldError::ManifestParse { path, message } => {
                write!(
                    f,
                    "failed to parse manifest '{}': {}",
                    path.display(),
                    message
                )
            }
            CbldError::LockParse { path, message } => {
                write!(
                    f,
                    "failed to parse lockfile '{}': {}",
                    path.display(),
                    message
                )
            }
            CbldError::Serialize(m) => write!(f, "failed to serialize: {}", m),
            CbldError::LayoutViolation(m) => write!(f, "project layout violation: {}", m),
            CbldError::NotCbldStandard { path, reason } => write!(
                f,
                "'{}' does not follow the cbld standard: {}",
                path.display(),
                reason
            ),
            CbldError::Resolution(m) => write!(f, "dependency resolution failed: {}", m),
            CbldError::CommandSpawn { program, source } => {
                write!(
                    f,
                    "failed to launch '{}': {} (is it installed and on PATH?)",
                    program, source
                )
            }
            CbldError::CommandFailed {
                program,
                code,
                stderr,
            } => {
                let code = code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into());
                write!(
                    f,
                    "'{}' exited with status {}:\n{}",
                    program,
                    code,
                    stderr.trim()
                )
            }
            CbldError::Compilation { failures, .. } => {
                write!(
                    f,
                    "build failed: {} translation unit(s) did not compile",
                    failures
                )
            }
            CbldError::Analysis { failures, .. } => {
                write!(
                    f,
                    "check failed: {} file(s) could not be analyzed",
                    failures
                )
            }
            CbldError::FmtCheck => {
                write!(
                    f,
                    "fmt --check failed: clang-format would rewrite one or more files"
                )
            }
            CbldError::Lint => {
                write!(f, "lint failed: clang-tidy reported findings")
            }
            CbldError::Config(m) => write!(f, "invalid configuration: {}", m),
            CbldError::Environment(m) => write!(f, "environment error: {}", m),
        }
    }
}

impl std::error::Error for CbldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CbldError::Io { source, .. } => Some(source),
            CbldError::CommandSpawn { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for CbldError {
    fn from(source: io::Error) -> Self {
        CbldError::Io { path: None, source }
    }
}

/// Convenience alias so signatures stay short.
pub type Result<T> = std::result::Result<T, CbldError>;

/// One structured compiler diagnostic, carried by `CbldError::Compilation` so
/// `--json` build output can render exactly what the terminal renderer
/// (`engine.rs`) shows, without re-parsing clang's stderr a second time.
#[derive(Debug, Clone)]
pub struct CompileDiagnostic {
    pub file: PathBuf,
    pub line: usize,
    pub column: usize,
    pub severity: &'static str,
    pub message: String,
}

/// Helper to attach a path to an io error after the fact.
pub trait IoPathExt<T> {
    fn path_ctx<P: Into<PathBuf>>(self, path: P) -> Result<T>;
}

impl<T> IoPathExt<T> for std::result::Result<T, io::Error> {
    fn path_ctx<P: Into<PathBuf>>(self, path: P) -> Result<T> {
        self.map_err(|source| CbldError::Io {
            path: Some(path.into()),
            source,
        })
    }
}
