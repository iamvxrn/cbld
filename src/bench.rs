//! Benchmark discovery and execution for `cbld bench`.
//!
//! The runner follows Google Benchmark's console format. It also accepts any
//! benchmark executable that follows the same six-column result shape, which
//! keeps the command useful for small custom runners without embedding a
//! benchmark framework.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::compiler::Language;
use crate::error::{CbldError, IoPathExt, Result};
use crate::manifest::Manifest;
use crate::test::{build_runner_binary, TestBuildOptions};

/// One parsed benchmark iteration row, with times normalized to nanoseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchmarkResult {
    pub name: String,
    pub real_time_ns: i64,
    pub cpu_time_ns: i64,
    pub iterations: i64,
}

/// Output and parsed rows from one benchmark process.
#[derive(Debug, Clone)]
pub struct BenchmarkRun {
    pub results: Vec<BenchmarkResult>,
    pub output: String,
}

/// Discover benchmark translation units under conventional benchmark paths.
pub fn collect_benchmark_sources(root: &Path) -> Result<Vec<PathBuf>> {
    let mut sources = Vec::new();
    for dir in ["benches", "benchmarks", "bench"] {
        let path = root.join(dir);
        if path.is_dir() {
            scan_recursive(&path, &mut sources)?;
        }
    }

    let src = root.join("src");
    if src.is_dir() {
        for entry in std::fs::read_dir(&src).path_ctx(&src)? {
            let path = entry.path_ctx(&src)?.path();
            if path.is_dir() {
                if path.file_name().and_then(|name| name.to_str()) == Some("benchmarks") {
                    scan_recursive(&path, &mut sources)?;
                }
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if (name.starts_with("bench_") || name.contains("_bench."))
                && Language::from_extension(&path).is_some()
            {
                sources.push(path);
            }
        }
    }

    sources.sort();
    sources.dedup();
    if sources.is_empty() {
        return Err(CbldError::LayoutViolation(format!(
            "no benchmark sources found under {}/benches, {}/benchmarks, {}/bench, or src/bench_*",
            root.display(),
            root.display(),
            root.display()
        )));
    }

    let mut c_seen = false;
    let mut cpp_seen = false;
    for source in &sources {
        match Language::from_extension(source) {
            Some(Language::C) => c_seen = true,
            Some(Language::Cpp) => cpp_seen = true,
            None => {}
        }
    }
    if c_seen && cpp_seen {
        return Err(CbldError::LayoutViolation(
            "benchmark suite mixes C and C++ sources".into(),
        ));
    }
    Ok(sources)
}

fn scan_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).path_ctx(dir)? {
        let path = entry.path_ctx(dir)?.path();
        if path.is_dir() {
            scan_recursive(&path, out)?;
        } else if Language::from_extension(&path).is_some() {
            out.push(path);
        }
    }
    Ok(())
}

/// Build a benchmark executable from discovered sources.
pub fn build_benchmarks(
    root: &Path,
    manifest: &Manifest,
    sources: &[PathBuf],
    options: TestBuildOptions<'_>,
) -> Result<PathBuf> {
    build_runner_binary(root, manifest, sources, options, "cbld-bench")
}

/// Run a Google Benchmark-compatible binary and parse its console rows.
pub fn run_benchmarks(bin_path: &Path, filter: Option<&str>) -> Result<BenchmarkRun> {
    let mut command = Command::new(bin_path);
    command.arg("--benchmark_format=console");
    if let Some(filter) = filter {
        command.arg(format!("--benchmark_filter={filter}"));
    }

    let output = command.output().map_err(|source| CbldError::CommandSpawn {
        program: bin_path.display().to_string(),
        source,
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{stdout}\n{stderr}");
    if !output.status.success() {
        return Err(CbldError::CommandFailed {
            program: bin_path.display().to_string(),
            code: output.status.code(),
            stderr: combined,
        });
    }

    let results = parse_console_output(&stdout);
    if results.is_empty() {
        return Err(CbldError::LayoutViolation(
            "benchmark executable produced no parseable result rows".into(),
        ));
    }
    Ok(BenchmarkRun {
        results,
        output: combined,
    })
}

/// Parse rows shaped like `name real unit cpu unit iterations`.
pub fn parse_console_output(output: &str) -> Vec<BenchmarkResult> {
    output.lines().filter_map(parse_console_line).collect()
}

fn parse_console_line(line: &str) -> Option<BenchmarkResult> {
    let mut fields = line.split_whitespace();
    let name = fields.next()?.to_string();
    let real_time = fields.next()?.parse::<f64>().ok()?;
    let real_unit = fields.next()?;
    let cpu_time = fields.next()?.parse::<f64>().ok()?;
    let cpu_unit = fields.next()?;
    let iterations = fields.next()?.parse::<i64>().ok()?;
    Some(BenchmarkResult {
        name,
        real_time_ns: to_nanoseconds(real_time, real_unit)?,
        cpu_time_ns: to_nanoseconds(cpu_time, cpu_unit)?,
        iterations,
    })
}

fn to_nanoseconds(value: f64, unit: &str) -> Option<i64> {
    if !value.is_finite() {
        return None;
    }
    let multiplier = match unit {
        "ns" => 1.0,
        "us" => 1_000.0,
        "ms" => 1_000_000.0,
        "s" => 1_000_000_000.0,
        _ => return None,
    };
    Some((value * multiplier).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_benchmark_directories_and_src_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("benchmarks/nested")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("benchmarks/nested/hash.cpp"), "").unwrap();
        fs::write(root.join("src/bench_sort.cpp"), "").unwrap();
        fs::write(root.join("src/main.cpp"), "").unwrap();

        let found = collect_benchmark_sources(root).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|path| path.ends_with("hash.cpp")));
        assert!(found.iter().any(|path| path.ends_with("bench_sort.cpp")));
    }

    #[test]
    fn rejects_mixed_benchmark_languages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("bench")).unwrap();
        fs::write(root.join("bench/a.c"), "").unwrap();
        fs::write(root.join("bench/b.cpp"), "").unwrap();
        assert!(collect_benchmark_sources(root).is_err());
    }

    #[test]
    fn parses_google_benchmark_rows_and_normalizes_units() {
        let output = "Benchmark             Time   CPU   Iterations\nBM_sort 1.5 us 2 ms 42\nBM_sort_mean 1 us 1 us";
        assert_eq!(
            parse_console_output(output),
            vec![BenchmarkResult {
                name: "BM_sort".into(),
                real_time_ns: 1_500,
                cpu_time_ns: 2_000_000,
                iterations: 42,
            }]
        );
    }

    #[test]
    fn ignores_unknown_units_and_malformed_rows() {
        let output = "BM_ok 1 ns 2 ns 3\nBM_bad 1 ticks 2 ns 3\nnot a row";
        let rows = parse_console_output(output);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "BM_ok");
    }
}
