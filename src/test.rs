//! Test discovery and runner for `cbld test`.
//! Scans for test sources, builds a test binary, and runs it.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::compiler::{Compiler, Language};
use crate::engine::{Crate, Engine};
use crate::error::{CbldError, IoPathExt, Result};
use crate::manifest::Manifest;

/// Collect all test sources for a package root.
/// Looks in:
/// - tests/**/*.c / *.cpp etc
/// - test/**/*.c / *.cpp
/// - src/test_*.c* , src/*_test.c* , src/tests/**/*.c*
pub fn collect_test_sources(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for dir in ["tests", "test"] {
        let p = root.join(dir);
        if p.is_dir() {
            out.extend(scan_test_dir(&p)?);
        }
    }
    let src = root.join("src");
    if src.is_dir() {
        out.extend(scan_src_tests(&src)?);
    }
    out.sort();
    out.dedup();
    if out.is_empty() {
        return Err(CbldError::LayoutViolation(format!(
            "no test sources found under {}/tests, {}/test, or src/test_*/src/*_test.*",
            root.display(),
            root.display()
        )));
    }
    Ok(out)
}

fn scan_test_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    scan_rec(dir, &mut out)?;
    Ok(out)
}

fn scan_src_tests(src: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    // Only files directly under src matching test_*.ext or *_test.ext, plus src/tests/*
    for entry in std::fs::read_dir(src).path_ctx(src)? {
        let path = entry.path_ctx(src)?.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("tests") {
                out.extend(scan_test_dir(&path)?);
            }
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_test = name.starts_with("test_")
            || name.contains("_test.")
            || name.starts_with("test-")
            || name.contains("-test.");
        if is_test && Language::from_extension(&path).is_some() {
            out.push(path);
        }
    }
    Ok(out)
}

fn scan_rec(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).path_ctx(dir)? {
        let path = entry.path_ctx(dir)?.path();
        if path.is_dir() {
            scan_rec(&path, out)?;
        } else if Language::from_extension(&path).is_some() {
            out.push(path);
        } else if Language::is_header(&path) {
            // headers not needed for test binary, skip
        }
    }
    Ok(())
}

/// Build a test binary from test sources.
/// Returns the path to the built executable.
pub struct TestBuildOptions<'a> {
    pub active_features: &'a [String],
    pub target: Option<&'a str>,
    pub sysroot: Option<&'a Path>,
    pub link_archives: &'a [PathBuf],
    pub release: bool,
    pub verbose: bool,
    pub quiet: bool,
    pub jobs: usize,
}

pub fn build_tests(
    root: &Path,
    manifest: &Manifest,
    test_sources: &[PathBuf],
    options: TestBuildOptions<'_>,
) -> Result<PathBuf> {
    build_runner_binary(root, manifest, test_sources, options, "cbld-test")
}

/// Build an executable from runner sources using the package's compiler setup.
/// The benchmark runner shares this path so test and benchmark builds stay
/// consistent with the package profiles and include paths.
pub(crate) fn build_runner_binary(
    root: &Path,
    manifest: &Manifest,
    test_sources: &[PathBuf],
    options: TestBuildOptions<'_>,
    binary_name: &str,
) -> Result<PathBuf> {
    let package = manifest
        .package
        .as_ref()
        .ok_or_else(|| CbldError::Config("no [package] in cbld.toml".into()))?;
    if test_sources.is_empty() {
        return Err(CbldError::LayoutViolation(
            "no test sources were discovered".into(),
        ));
    }
    let target_dir = crate::engine::target_dir(root, options.target).join(if options.release {
        "release"
    } else {
        "debug"
    });
    std::fs::create_dir_all(&target_dir).path_ctx(&target_dir)?;
    let bin_path = target_dir.join(if cfg!(target_os = "windows") {
        format!("{binary_name}.exe")
    } else {
        binary_name.to_string()
    });

    // Use the same compiler setup as normal builds but force executable
    let c_profile = manifest.profile.c.clone().unwrap_or_default();
    let cpp_profile = manifest.profile.cpp.clone().unwrap_or_default();
    let include_dirs = package
        .include_dirs
        .iter()
        .map(|dir| root.join(dir))
        .collect();
    let compiler = Compiler::new_with_sysroot(
        c_profile,
        cpp_profile,
        root,
        include_dirs,
        options.active_features,
        options.release,
        false,
        options.target.map(str::to_owned),
        options.sysroot.map(Path::to_path_buf),
        package.defines.clone(),
        package.ignore_warnings,
    );
    compiler.validate()?;

    // Build a synthetic executable layout for runner sources. We bypass Layout
    // because test and benchmark trees are intentionally outside src/.
    let engine = Engine::new(options.jobs, options.verbose, options.quiet, false, false);
    let mut units = Vec::new();
    let obj_dir = target_dir.join("obj").join(binary_name);
    std::fs::create_dir_all(&obj_dir).path_ctx(&obj_dir)?;
    for src in test_sources {
        let relative = src.strip_prefix(root).unwrap_or(src);
        let mut obj = obj_dir.join(relative);
        obj.set_extension("o");
        if let Some(parent) = obj.parent() {
            std::fs::create_dir_all(parent).path_ctx(parent)?;
        }
        let unit = compiler.compile_unit(src, &obj)?;
        units.push(unit);
    }
    // Also need language for linking: if any cpp, use cpp
    let has_cpp = test_sources
        .iter()
        .any(|p| Language::from_extension(p) == Some(Language::Cpp));
    // Compile
    let objects = engine.compile_for_test(units)?;
    // Link
    let mut link_inputs = objects;
    link_inputs.extend(options.link_archives.iter().cloned());
    let links = compiler.link_command(
        &link_inputs,
        &bin_path,
        has_cpp,
        false,
        false,
        &package.libs,
    );
    engine.run_link_for_test(&links, Crate::Executable)?;

    Ok(bin_path)
}

/// Run the test binary and return (passed, failed, output).
pub fn run_tests(bin_path: &Path, filter: Option<&str>) -> Result<(usize, usize, String)> {
    let mut cmd = Command::new(bin_path);
    if let Some(f) = filter {
        // Best-effort: pass as gtest filter and plain arg
        cmd.arg(format!("--gtest_filter={f}"));
        // Many frameworks also accept plain filter
        // We do not double-add; the binary will ignore unknown flags
    }
    let out = cmd.output().map_err(|e| CbldError::CommandSpawn {
        program: bin_path.display().to_string(),
        source: e,
    })?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let combined = format!("{stdout}\n{stderr}");
    // Parse the common summaries; a plain executable still gets a useful
    // one-pass/one-failure result from its exit status.
    let (passed, failed) = parse_test_summary(&combined, out.status.success());
    Ok((passed, failed, combined))
}

fn parse_test_summary(output: &str, success: bool) -> (usize, usize) {
    let mut passed = None;
    let mut failed = None;
    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("passed") {
            passed = find_count_before(line, "passed").or_else(|| find_count_after_bracket(line));
        }
        if lower.contains("failed") {
            failed = find_count_before(line, "failed").or_else(|| find_count_after_bracket(line));
        }
    }
    if passed.is_some() || failed.is_some() {
        return (passed.unwrap_or(0), failed.unwrap_or(0));
    }
    if success {
        (1, 0)
    } else {
        (0, 1)
    }
}

fn find_count_before(line: &str, label: &str) -> Option<usize> {
    let tokens: Vec<_> = line.split_whitespace().collect();
    tokens.windows(2).find_map(|window| {
        let normalized = window[1].trim_matches(|c: char| !c.is_ascii_alphabetic());
        (normalized.eq_ignore_ascii_case(label))
            .then(|| window[0].parse::<usize>().ok())
            .flatten()
    })
}

fn find_count_after_bracket(line: &str) -> Option<usize> {
    line.split_once(']')?
        .1
        .split_whitespace()
        .find_map(|token| token.parse::<usize>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn discovers_tests_in_tests_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(&root.join("tests/test_foo.cpp"), "int main(){return 0;}");
        write_file(&root.join("tests/bar_test.cpp"), "int main(){return 0;}");
        write_file(&root.join("tests/sub/baz.cpp"), "int main(){return 0;}");
        let found = collect_test_sources(root).unwrap();
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn discovers_src_test_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        write_file(&root.join("src/test_foo.cpp"), "int main(){return 0;}");
        write_file(&root.join("src/my_test.cpp"), "int main(){return 0;}");
        write_file(&root.join("src/lib.cpp"), "int x=0;");
        let found = collect_test_sources(root).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|p| p.ends_with("test_foo.cpp")));
    }

    #[test]
    fn errors_when_no_tests() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        write_file(&root.join("src/lib.cpp"), "int x=0;");
        assert!(collect_test_sources(root).is_err());
    }

    #[test]
    fn accepts_mixed_c_and_cpp() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(&root.join("tests/a.cpp"), "int main(){return 0;}");
        write_file(&root.join("tests/b.c"), "int main(){return 0;}");
        assert_eq!(collect_test_sources(root).unwrap().len(), 2);
    }

    #[test]
    fn parses_gtest_summary() {
        assert_eq!(parse_test_summary("[  PASSED  ] 3 tests.", true), (3, 0));
        assert_eq!(parse_test_summary("[  FAILED  ] 1 test.", false), (0, 1));
    }

    #[test]
    fn parses_catch_summary() {
        assert_eq!(
            parse_test_summary("test cases: 3 | 2 passed | 1 failed", false),
            (2, 1)
        );
    }
}
