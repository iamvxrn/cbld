//! Clang `-ftime-trace` orchestration for `cbld build --trace`.
//!
//! Clang (since version 9) writes `-ftime-trace`'s output next to the object
//! file, reusing its basename with a `.json` extension — cbld relies on that
//! convention rather than passing `-ftime-trace=<path>` explicitly, since
//! every object file already has a unique flattened basename
//! (`object_path()` in engine.rs). Once a package finishes compiling, this
//! module scans that package's object directory for the per-unit trace
//! files clang left behind, merges them into one `cbld_profile.json` (Chrome
//! Trace Event Format — loadable directly at chrome://tracing or
//! speedscope.app), and prints a terminal summary of the slowest individual
//! headers/templates across the whole package.

use std::fs;
use std::path::{Path, PathBuf};

use crate::json::Json;

/// One duration event with a concrete `detail` (a header path, a template
/// instantiation's symbol name, ...) — the granular events worth surfacing
/// in a bottleneck summary, as opposed to umbrella events like
/// `ExecuteCompiler`/`Frontend`/`Backend` that just sum up everything below
/// them.
struct Bottleneck {
    source_file: String,
    name: String,
    detail: String,
    dur_us: i64,
}

/// Merge every `*.json` trace file sitting in `obj_dir` into a single
/// `cbld_profile.json` under `profile_dir`, print the top bottlenecks to the
/// terminal (unless `quiet`), then remove the individual per-unit files.
///
/// Best-effort throughout: a missing, empty, or malformed trace file is
/// skipped rather than failing the build — profiling is a diagnostic aid,
/// not a build correctness concern, so this function never returns an
/// error.
pub fn aggregate_and_report(obj_dir: &Path, profile_dir: &Path, quiet: bool) {
    let Ok(dir_entries) = fs::read_dir(obj_dir) else {
        return;
    };

    let mut merged_events: Vec<Json> = Vec::new();
    let mut bottlenecks: Vec<Bottleneck> = Vec::new();
    let mut trace_files: Vec<PathBuf> = Vec::new();

    for (pid, entry) in dir_entries.flatten().enumerate() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = Json::parse(&raw) else {
            continue;
        };
        let Some(events) = doc.get("traceEvents").and_then(Json::as_array) else {
            continue;
        };

        let source_file = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        // A synthetic `process_name` metadata event so chrome://tracing and
        // speedscope group this translation unit's events into their own
        // track instead of colliding with every other unit's pid/tid.
        merged_events.push(Json::Object(vec![
            ("pid".to_string(), Json::Number(pid as i64)),
            ("tid".to_string(), Json::Number(0)),
            ("ph".to_string(), Json::str("M")),
            ("name".to_string(), Json::str("process_name")),
            (
                "args".to_string(),
                Json::Object(vec![("name".to_string(), Json::str(source_file.clone()))]),
            ),
        ]));

        for event in events {
            if event.get("ph").and_then(Json::as_str) != Some("X") {
                continue;
            }
            let name = event.get("name").and_then(Json::as_str).unwrap_or("?");
            let dur_us = event.get("dur").and_then(Json::as_i64).unwrap_or(0);
            let ts = event.get("ts").and_then(Json::as_i64).unwrap_or(0);
            let detail = event
                .get("args")
                .and_then(|a| a.get("detail"))
                .and_then(Json::as_str);

            if let Some(detail) = detail {
                bottlenecks.push(Bottleneck {
                    source_file: source_file.clone(),
                    name: name.to_string(),
                    detail: detail.to_string(),
                    dur_us,
                });
            }

            let mut fields = vec![
                ("pid".to_string(), Json::Number(pid as i64)),
                ("tid".to_string(), Json::Number(0)),
                ("ph".to_string(), Json::str("X")),
                ("ts".to_string(), Json::Number(ts)),
                ("dur".to_string(), Json::Number(dur_us)),
                ("name".to_string(), Json::str(name.to_string())),
            ];
            if let Some(detail) = detail {
                fields.push((
                    "args".to_string(),
                    Json::Object(vec![("detail".to_string(), Json::str(detail.to_string()))]),
                ));
            }
            merged_events.push(Json::Object(fields));
        }

        trace_files.push(path);
    }

    if merged_events.is_empty() {
        return;
    }

    let doc = Json::Object(vec![(
        "traceEvents".to_string(),
        Json::Array(merged_events),
    )]);
    let out_path = profile_dir.join("cbld_profile.json");
    if fs::write(&out_path, doc.render()).is_err() {
        return;
    }

    // "Aggregate ... into a single ... profile": the per-unit files are now
    // redundant, so fold them into the merged one rather than leaving both
    // around.
    for f in &trace_files {
        let _ = fs::remove_file(f);
    }

    if !quiet {
        print_summary(&bottlenecks, &out_path);
    }
}

fn print_summary(bottlenecks: &[Bottleneck], out_path: &Path) {
    let mut sorted: Vec<&Bottleneck> = bottlenecks.iter().collect();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.dur_us));
    let shown = sorted.len().min(10);

    println!(
        "\x1b[1;36m    Profile\x1b[0m top {} compilation bottleneck{}",
        shown,
        if shown == 1 { "" } else { "s" }
    );
    for b in sorted.iter().take(10) {
        let ms = b.dur_us as f64 / 1000.0;
        println!(
            "        {:>8.2}ms  {:<20} {}  \x1b[2m(in {})\x1b[0m",
            ms, b.name, b.detail, b.source_file
        );
    }
    println!(
        "\x1b[1;32m    Finished\x1b[0m profile written to {}",
        out_path.display()
    );
    println!("               load it at chrome://tracing or https://www.speedscope.app");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cbld-trace-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_trace_file(path: &Path, events_json: &str) {
        std::fs::write(path, format!(r#"{{"traceEvents": [{events_json}]}}"#)).unwrap();
    }

    #[test]
    fn aggregates_multiple_unit_traces_into_one_profile_and_removes_originals() {
        let obj_dir = temp_dir("agg-obj");
        let profile_dir = temp_dir("agg-profile");

        write_trace_file(
            &obj_dir.join("main__cpp.json"),
            r#"{"pid":1,"tid":0,"ph":"X","ts":0,"dur":500,"name":"Source","args":{"detail":"a.h"}}"#,
        );
        write_trace_file(
            &obj_dir.join("util__cpp.json"),
            r#"{"pid":1,"tid":0,"ph":"X","ts":0,"dur":9000,"name":"InstantiateFunction","args":{"detail":"foo<int>"}}"#,
        );

        aggregate_and_report(&obj_dir, &profile_dir, true);

        assert!(!obj_dir.join("main__cpp.json").exists());
        assert!(!obj_dir.join("util__cpp.json").exists());

        let merged_path = profile_dir.join("cbld_profile.json");
        assert!(merged_path.is_file());
        let merged = std::fs::read_to_string(&merged_path).unwrap();
        let doc = Json::parse(&merged).unwrap();
        let events = doc.get("traceEvents").and_then(Json::as_array).unwrap();
        // 2 process_name metadata events + 2 real duration events.
        assert_eq!(events.len(), 4);

        let _ = std::fs::remove_dir_all(&obj_dir);
        let _ = std::fs::remove_dir_all(&profile_dir);
    }

    #[test]
    fn missing_obj_dir_is_a_silent_no_op() {
        let missing = std::env::temp_dir().join("cbld-trace-test-does-not-exist");
        let profile_dir = temp_dir("noop-profile");
        aggregate_and_report(&missing, &profile_dir, true);
        assert!(!profile_dir.join("cbld_profile.json").exists());
        let _ = std::fs::remove_dir_all(&profile_dir);
    }

    #[test]
    fn malformed_trace_file_is_skipped_not_fatal() {
        let obj_dir = temp_dir("malformed-obj");
        let profile_dir = temp_dir("malformed-profile");
        std::fs::write(obj_dir.join("broken.json"), "{not valid json").unwrap();

        aggregate_and_report(&obj_dir, &profile_dir, true);
        assert!(!profile_dir.join("cbld_profile.json").exists());

        let _ = std::fs::remove_dir_all(&obj_dir);
        let _ = std::fs::remove_dir_all(&profile_dir);
    }
}
