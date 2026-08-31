//! Test discovery for `cbld test`.
//! Scans for test sources without touching the build graph.

use std::path::{Path, PathBuf};

use crate::compiler::Language;
use crate::error::{CbldError, IoPathExt, Result};

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
    // Enforce single-language per test suite (same as package)
    let mut c_seen = false;
    let mut cpp_seen = false;
    for p in &out {
        match Language::from_extension(p) {
            Some(Language::C) => c_seen = true,
            Some(Language::Cpp) => cpp_seen = true,
            _ => {}
        }
    }
    if c_seen && cpp_seen {
        return Err(CbldError::LayoutViolation(
            "test suite mixes C and C++ sources — split or filter with include/exclude".into(),
        ));
    }
    Ok(out)
}

fn scan_test_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    scan_rec(dir, dir, &mut out)?;
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
        let is_test = name.starts_with("test_") || name.contains("_test.")
            || name.starts_with("test-") || name.contains("-test.");
        if is_test && Language::from_extension(&path).is_some() {
            out.push(path);
        }
    }
    Ok(out)
}

fn scan_rec(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).path_ctx(dir)? {
        let path = entry.path_ctx(dir)?.path();
        if path.is_dir() {
            scan_rec(base, &path, out)?;
        } else if Language::from_extension(&path).is_some() {
            out.push(path);
        } else if Language::is_header(&path) {
            // headers not needed for test binary, skip
        }
    }
    Ok(())
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
    fn rejects_mixed_c_and_cpp() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(&root.join("tests/a.cpp"), "int main(){return 0;}");
        write_file(&root.join("tests/b.c"), "int main(){return 0;}");
        assert!(collect_test_sources(root).is_err());
    }
}
