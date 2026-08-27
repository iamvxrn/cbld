//! `cbld fmt` — clang-format over the package's sources and public headers.
//!
//! Same role as `cargo fmt`: rewrite in place, or `--check` and fail if
//! anything would change. Style comes from `.clang-format` when present
//! (`-style=file`); otherwise LLVM, clang-format's own default.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;

use crate::engine::Layout;
use crate::error::{CbldError, Result};

/// Run clang-format on every file [`Layout::collect_format_files`] returns.
pub fn run(layout: &Layout, check: bool, verbose: bool, quiet: bool) -> Result<()> {
    let files = layout.collect_format_files()?;
    let bin = require_clang_format()?;

    if !quiet {
        let verb = if check { "Checking" } else { "Formatting" };
        println!(
            "\x1b[1;36m   {verb}\x1b[0m {} file{}",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
    }
    if verbose {
        for f in &files {
            eprintln!("  \x1b[2m[fmt]\x1b[0m {}", f.display());
        }
    }

    let args = format_invocation(&files, check);
    let status =
        Command::new(&bin)
            .args(&args)
            .status()
            .map_err(|source| CbldError::CommandSpawn {
                program: bin.clone(),
                source,
            })?;

    if status.success() {
        if !quiet && !check {
            println!(
                "\x1b[1;32m    Finished\x1b[0m {} file{}",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            );
        }
        return Ok(());
    }

    if check {
        return Err(CbldError::FmtCheck);
    }
    Err(CbldError::CommandFailed {
        program: bin,
        code: status.code(),
        stderr: String::new(),
    })
}

pub(crate) fn format_invocation(files: &[PathBuf], check: bool) -> Vec<String> {
    let mut args = vec![
        "-style=file".to_string(),
        "-fallback-style=LLVM".to_string(),
    ];
    if check {
        args.push("--dry-run".to_string());
        args.push("--Werror".to_string());
    } else {
        args.push("-i".to_string());
    }
    args.extend(files.iter().map(|p| p.display().to_string()));
    args
}

fn require_clang_format() -> Result<String> {
    for name in ["clang-format", "clang-format.exe"] {
        match Command::new(name).arg("--version").output() {
            Ok(_) => return Ok(name.to_string()),
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(_) => return Ok(name.to_string()),
        }
    }
    Err(CbldError::Environment(
        "clang-format not found on PATH (install LLVM; `cbld doctor` lists it)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn check_invocation_uses_dry_run_and_werror() {
        let files = [PathBuf::from("src/main.cpp")];
        let args = format_invocation(&files, true);
        assert!(args.contains(&"--dry-run".to_string()));
        assert!(args.contains(&"--Werror".to_string()));
        assert!(!args.iter().any(|a| a == "-i"));
        assert_eq!(args.last().map(String::as_str), Some("src/main.cpp"));
    }

    #[test]
    fn write_invocation_uses_in_place() {
        let files = [PathBuf::from("include/foo.h"), PathBuf::from("src/a.c")];
        let args = format_invocation(&files, false);
        assert!(args.contains(&"-i".to_string()));
        assert!(!args.iter().any(|a| a == "--dry-run"));
        assert_eq!(args[args.len() - 2], "include/foo.h");
        assert_eq!(args.last().map(String::as_str), Some("src/a.c"));
    }

    #[test]
    fn fmt_check_fails_until_the_file_is_rewritten() {
        if Command::new("clang-format")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        use crate::engine::{Layout, ScanConfig};
        use std::fs;

        let tmp = std::env::temp_dir().join(format!("cbld-fmt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();
        fs::write(tmp.join("src").join("main.cpp"), "int main(){return 0;}\n").unwrap();
        let layout = Layout::discover(&tmp, &ScanConfig::strict("src")).unwrap();
        assert!(run(&layout, true, false, true).is_err());
        run(&layout, false, false, true).unwrap();
        run(&layout, true, false, true).unwrap();
        let _ = fs::remove_dir_all(&tmp);
    }
}
