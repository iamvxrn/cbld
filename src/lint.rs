//! `cbld lint` — clang-tidy over the same files `cbld fmt` touches.
//!
//! The cargo-clippy analog: extra lints beyond what `cbld build` already
//! reports as compiler warnings. `--deny-warnings` is clippy's `-D warnings`
//! — any finding fails the command. `cbld check` stays Clang's static
//! analyzer (`--analyze`); this command is tidy.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;

use crate::compiler::Compiler;
use crate::engine::Layout;
use crate::error::{CbldError, Result};

/// Default tidy checks when the package has no `.clang-tidy`. Diagnostic
/// groups that overlap the compiler, plus bugprone/performance — useful
/// without turning on the entire `modernize-*` rewrite set.
const DEFAULT_CHECKS: &str = "clang-diagnostic-*,bugprone-*,performance-*";

/// Run clang-tidy. `compiler` supplies include paths and language dialect
/// so tidy parses the same way `cbld build` would.
pub fn run(
    layout: &Layout,
    compiler: &Compiler,
    deny_warnings: bool,
    verbose: bool,
    quiet: bool,
) -> Result<()> {
    let files = layout.collect_format_files()?;
    let bin = require_clang_tidy()?;
    let use_default_checks = !layout.root.join(".clang-tidy").is_file();
    let header_filter = regex_escape(&layout.root.to_string_lossy().replace('\\', "/"));
    let frontend = compiler.tidy_frontend_args(layout.entry_language);
    let args = lint_invocation(
        &files,
        deny_warnings,
        use_default_checks,
        &header_filter,
        &frontend,
    );

    if !quiet {
        println!(
            "\x1b[1;36m     Linting\x1b[0m {} file{}",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
    }
    if verbose {
        eprintln!("  \x1b[2m[lint]\x1b[0m {bin} {}", args.join(" "));
    }

    let status =
        Command::new(&bin)
            .args(&args)
            .status()
            .map_err(|source| CbldError::CommandSpawn {
                program: bin.clone(),
                source,
            })?;

    if status.success() {
        if !quiet {
            println!(
                "\x1b[1;32m    Finished\x1b[0m {} file{}",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            );
        }
        return Ok(());
    }

    if deny_warnings {
        return Err(CbldError::Lint);
    }
    // Without `--deny-warnings`, a non-zero tidy exit is still a failure:
    // it means a file could not be parsed, not merely that lints fired.
    Err(CbldError::Lint)
}

pub(crate) fn lint_invocation(
    files: &[PathBuf],
    deny_warnings: bool,
    use_default_checks: bool,
    header_filter: &str,
    compiler_args: &[String],
) -> Vec<String> {
    let mut args = vec![
        "-quiet".to_string(),
        format!("-header-filter={header_filter}"),
    ];
    if use_default_checks {
        args.push(format!("-checks={DEFAULT_CHECKS}"));
    }
    if deny_warnings {
        args.push("-warnings-as-errors=*".to_string());
    }
    args.extend(files.iter().map(|p| p.display().to_string()));
    args.push("--".to_string());
    args.extend(compiler_args.iter().cloned());
    args
}

fn require_clang_tidy() -> Result<String> {
    for name in ["clang-tidy", "clang-tidy.exe"] {
        match Command::new(name).arg("--version").output() {
            Ok(_) => return Ok(name.to_string()),
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(_) => return Ok(name.to_string()),
        }
    }
    Err(CbldError::Environment(
        "clang-tidy not found on PATH (install LLVM; `cbld doctor` lists it)".into(),
    ))
}

/// Escape a path so it is safe inside clang-tidy's `-header-filter` regex.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ".+*?^$()[]{}|\\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_warnings_passes_warnings_as_errors() {
        let files = [PathBuf::from("src/main.cpp")];
        let args = lint_invocation(&files, true, true, "/proj", &["-std=c++20".into()]);
        assert!(args.iter().any(|a| a == "-warnings-as-errors=*"));
        assert!(args.iter().any(|a| a.starts_with("-checks=")));
        let sep = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[sep - 1], "src/main.cpp");
        assert_eq!(args[sep + 1], "-std=c++20");
    }

    #[test]
    fn existing_clang_tidy_config_skips_default_checks() {
        let files = [PathBuf::from("src/a.c")];
        let args = lint_invocation(&files, false, false, "/proj", &[]);
        assert!(!args.iter().any(|a| a.starts_with("-checks=")));
        assert!(!args.iter().any(|a| a == "-warnings-as-errors=*"));
    }

    #[test]
    fn regex_escape_dots_in_paths() {
        assert_eq!(regex_escape("/home/a.b/c"), "/home/a\\.b/c");
    }

    #[test]
    fn lint_runs_on_a_trivial_cpp_file() {
        if Command::new("clang-tidy")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        use crate::compiler::Compiler;
        use crate::engine::{Layout, ScanConfig};
        use crate::manifest::{CProfile, CppProfile};
        use std::fs;

        let tmp = std::env::temp_dir().join(format!("cbld-lint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();
        fs::write(
            tmp.join("src").join("main.cpp"),
            "int main() { return 0; }\n",
        )
        .unwrap();
        let layout = Layout::discover(&tmp, &ScanConfig::strict("src")).unwrap();
        let compiler = Compiler::new(
            CProfile::default(),
            CppProfile::default(),
            &tmp,
            Vec::new(),
            &[],
            false,
            false,
            None,
            Vec::new(),
            false,
        );
        run(&layout, &compiler, false, false, true).unwrap();
        let _ = fs::remove_dir_all(&tmp);
    }
}
