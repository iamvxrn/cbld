//! `cbld doctor` — environment and toolchain diagnostics.
//!
//! These checks are deliberately kept OUT of `cbld build`'s hot path: a build
//! that already has a valid environment should not pay for probing it on
//! every invocation. Diagnostics run only when the user explicitly asks
//! (`cbld doctor`) or when a build has just failed, at which point a slow,
//! thorough check is the right trade — the user is already stopped and wants
//! to know why.

use std::io::ErrorKind;
use std::process::Command;

use crate::error::Result;
use crate::json::Json;
use crate::manifest::{Manifest, ToolchainSpec};

/// One diagnostic check and its outcome.
struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
    fix: Option<String>,
}

/// Run every diagnostic and print a report — human-readable by default, or a
/// single structured JSON object on stdout when `json` is set.
///
/// Returns `Ok(())` even when checks fail — `doctor` is a report, not a gate.
/// Callers that care about pass/fail can inspect the process exit code via
/// [`any_critical_failed`]-style logic if ever needed; today we keep it simple
/// and always exit 0 after printing, since the report itself is the value.
pub fn run(verbose: bool, json: bool) -> Result<()> {
    let mut checks = vec![
        check_compiler("clang", "C"),
        check_compiler("clang++", "C++"),
        check_archiver(),
        check_git(),
        check_fetch_tool(),
        check_system_headers(),
        check_cbld_home(),
    ];
    if let Some(toolchain) = check_toolchain_pin() {
        checks.push(toolchain);
    }

    if json {
        print_json_report(&checks);
    } else {
        print_report(&checks, verbose);
    }
    Ok(())
}

fn print_report(checks: &[Check], verbose: bool) {
    println!("\x1b[1;36mcbld doctor\x1b[0m — environment diagnostics\n");

    let mut passed = 0usize;
    for c in checks {
        if c.ok {
            passed += 1;
            println!("  \x1b[1;32m[ ok ]\x1b[0m {:<14} {}", c.name, c.detail);
        } else {
            println!("  \x1b[1;31m[fail]\x1b[0m {:<14} {}", c.name, c.detail);
            if let Some(fix) = &c.fix {
                println!("           \x1b[2mfix:\x1b[0m {fix}");
            }
        }
    }

    let failed = checks.len() - passed;
    println!();
    if failed == 0 {
        println!(
            "\x1b[1;32mAll {} checks passed.\x1b[0m Your environment is ready for `cbld build`.",
            checks.len()
        );
    } else {
        println!(
            "\x1b[1;33m{passed} passed, {failed} failed.\x1b[0m Fix the items above, then re-run `cbld doctor`."
        );
    }
    if verbose {
        eprintln!("  \x1b[2m[doctor]\x1b[0m ran {} check(s)", checks.len());
    }
}

/// Build the same checks as a single JSON object: an array of `{name, ok,
/// detail, fix}` records plus a `passed`/`failed` tally — the "environment
/// check matrix" CI tooling can parse directly.
fn build_json_report(checks: &[Check]) -> Json {
    let passed = checks.iter().filter(|c| c.ok).count();
    let items: Vec<Json> = checks
        .iter()
        .map(|c| {
            Json::Object(vec![
                ("name".to_string(), Json::str(c.name)),
                ("ok".to_string(), Json::Bool(c.ok)),
                ("detail".to_string(), Json::str(c.detail.clone())),
                (
                    "fix".to_string(),
                    match &c.fix {
                        Some(f) => Json::str(f.clone()),
                        None => Json::Null,
                    },
                ),
            ])
        })
        .collect();

    Json::Object(vec![
        ("checks".to_string(), Json::Array(items)),
        ("passed".to_string(), Json::Number(passed as i64)),
        (
            "failed".to_string(),
            Json::Number((checks.len() - passed) as i64),
        ),
    ])
}

fn print_json_report(checks: &[Check]) {
    println!("{}", build_json_report(checks).render());
}

/// If the current directory has a `cbld.toml` with a `[package] toolchain`
/// pin, validate it against the active compiler. Returns `None` (no check
/// emitted) when there's no project here or no pin declared — `doctor`
/// otherwise stays project-agnostic, matching every other check above.
fn check_toolchain_pin() -> Option<Check> {
    let root = std::env::current_dir().ok()?;
    let manifest = Manifest::load(&root).ok()?;
    let spec_str = manifest.package?.toolchain?;

    let spec = match ToolchainSpec::parse(&spec_str) {
        Ok(s) => s,
        Err(e) => {
            return Some(Check {
                name: "toolchain",
                ok: false,
                detail: format!("invalid toolchain spec: {e}"),
                fix: Some("fix the `toolchain` value in [package] (e.g. \"clang-18.1\")".into()),
            });
        }
    };

    Some(match spec.validate() {
        Ok(()) => Check {
            name: "toolchain",
            ok: true,
            detail: format!("pinned to {}-{} and satisfied", spec.compiler, spec.version),
            fix: None,
        },
        Err(e) => Check {
            name: "toolchain",
            ok: false,
            detail: e.to_string(),
            fix: Some(format!(
                "install/select {} {} (or update [package] toolchain in cbld.toml)",
                spec.compiler, spec.version
            )),
        },
    })
}

/// Probe a clang driver for presence and version.
fn check_compiler(driver: &'static str, label: &str) -> Check {
    match Command::new(driver).arg("--version").output() {
        Ok(out) if out.status.success() => {
            let first_line = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            Check {
                name: driver,
                ok: true,
                detail: first_line,
                fix: None,
            }
        }
        _ => Check {
            name: driver,
            ok: false,
            detail: format!("not found on PATH ({label} compiler required)"),
            fix: Some(install_hint_clang()),
        },
    }
}

fn check_archiver() -> Check {
    // Same preference order as `archiver_candidates` in compiler.rs:
    // llvm-ar (macOS/Windows), then system `ar`, then MSVC `lib.exe`.
    for name in ["llvm-ar", "ar", "lib.exe"] {
        if name == "lib.exe" && cfg!(not(target_os = "windows")) {
            continue;
        }
        if !tool_on_path(name) {
            continue;
        }
        let detail = archiver_version_line(name).unwrap_or_else(|| format!("{name} found on PATH"));
        return Check {
            name: "archiver",
            ok: true,
            detail,
            fix: None,
        };
    }
    Check {
        name: "archiver",
        ok: false,
        detail: "no archiver on PATH (need llvm-ar, ar, or lib.exe)".to_string(),
        fix: Some(install_hint_binutils()),
    }
}

/// `Command::new` only yields NotFound when the binary is absent; any other
/// result (including a non-zero `--version`) means the tool exists.
fn tool_on_path(name: &str) -> bool {
    match Command::new(name).arg("--version").output() {
        Ok(_) => true,
        Err(e) => e.kind() != ErrorKind::NotFound,
    }
}

fn archiver_version_line(name: &str) -> Option<String> {
    let out = Command::new(name).arg("--version").output().ok()?;
    let text = if out.status.success() {
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

fn check_git() -> Check {
    match Command::new("git").arg("--version").output() {
        Ok(out) if out.status.success() => Check {
            name: "git",
            ok: true,
            detail: String::from_utf8_lossy(&out.stdout).trim().to_string(),
            fix: None,
        },
        _ => Check {
            name: "git",
            ok: false,
            detail: "not found on PATH (required to resolve gh: dependencies)".to_string(),
            fix: Some("install git (e.g. `sudo apt install git`, `brew install git`)".into()),
        },
    }
}

/// `curl` is preferred, `wget` and PowerShell's `Invoke-WebRequest` (on
/// Windows) are accepted fallbacks for package index syncing.
fn check_fetch_tool() -> Check {
    if cfg!(target_os = "windows") {
        return match Command::new("powershell")
            .args(["-NoProfile", "-Command", "$PSVersionTable.PSVersion"])
            .output()
        {
            Ok(out) if out.status.success() => Check {
                name: "powershell",
                ok: true,
                detail: "available for index sync (Invoke-WebRequest)".to_string(),
                fix: None,
            },
            _ => Check {
                name: "powershell",
                ok: false,
                detail: "not found (required to sync the package index on Windows)".to_string(),
                fix: Some("ensure PowerShell is installed and on PATH".into()),
            },
        };
    }

    if Command::new("curl")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return Check {
            name: "curl",
            ok: true,
            detail: "available for index sync".to_string(),
            fix: None,
        };
    }
    if Command::new("wget")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return Check {
            name: "wget",
            ok: true,
            detail: "available for index sync (curl not found, using fallback)".to_string(),
            fix: None,
        };
    }
    Check {
        name: "curl/wget",
        ok: false,
        detail: "neither curl nor wget found on PATH".to_string(),
        fix: Some(
            "install curl (e.g. `sudo apt install curl`, `brew install curl`) — \
             required for `cbld sync` and dependency reachability probing"
                .into(),
        ),
    }
}

/// Confirm clang can actually find standard headers and produce an object
/// file end-to-end — the real-world test that "compiler present" doesn't
/// cover (a broken sysroot or missing libc headers still fails builds).
fn check_system_headers() -> Check {
    let dir = std::env::temp_dir();
    let probe_src = dir.join(format!("cbld-doctor-{}.c", std::process::id()));
    let probe_obj = probe_src.with_extension("o");

    if std::fs::write(
        &probe_src,
        "#include <stdio.h>\nint main(void){return 0;}\n",
    )
    .is_err()
    {
        return Check {
            name: "headers",
            ok: false,
            detail: "could not write a probe file to the temp directory".to_string(),
            fix: Some("ensure the temp directory is writable".into()),
        };
    }

    let result = Command::new("clang")
        .args([
            "-c",
            probe_src.to_string_lossy().as_ref(),
            "-o",
            probe_obj.to_string_lossy().as_ref(),
        ])
        .output();

    let _ = std::fs::remove_file(&probe_src);
    let _ = std::fs::remove_file(&probe_obj);

    match result {
        Ok(out) if out.status.success() => Check {
            name: "headers",
            ok: true,
            detail: "stdio.h resolved and compiled cleanly".to_string(),
            fix: None,
        },
        Ok(out) => Check {
            name: "headers",
            ok: false,
            detail: "clang could not compile a trivial program".to_string(),
            fix: Some(format!(
                "check your sysroot/include paths; clang said: {}",
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
            )),
        },
        Err(_) => Check {
            name: "headers",
            ok: false,
            detail: "skipped — clang itself is unavailable".to_string(),
            fix: Some(install_hint_clang()),
        },
    }
}

fn check_cbld_home() -> Check {
    match crate::resolver::cbld_home() {
        Err(e) => Check {
            name: "cbld home",
            ok: false,
            detail: e.to_string(),
            fix: Some("set HOME, USERPROFILE, or CBLD_HOME so cbld can locate its cache".into()),
        },
        Ok(home) => {
            if home.is_dir() {
                Check {
                    name: "cbld home",
                    ok: true,
                    detail: home.display().to_string(),
                    fix: None,
                }
            } else {
                Check {
                    name: "cbld home",
                    ok: true,
                    detail: format!("{} (will be created on first build)", home.display()),
                    fix: None,
                }
            }
        }
    }
}

fn install_hint_clang() -> String {
    match std::env::consts::OS {
        "macos" => "install LLVM: `brew install llvm`".to_string(),
        "windows" => "install LLVM: `winget install LLVM.LLVM`".to_string(),
        _ => "install clang: `sudo apt install clang` (or your distro's equivalent)".to_string(),
    }
}

fn install_hint_binutils() -> String {
    match std::env::consts::OS {
        "macos" => "install binutils: `brew install binutils`".to_string(),
        "windows" => "install LLVM, which ships `llvm-ar`, or MSYS2 binutils".to_string(),
        _ => "install binutils: `sudo apt install binutils`".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_checks() -> Vec<Check> {
        vec![
            Check {
                name: "clang",
                ok: true,
                detail: "clang version 18.1.3".to_string(),
                fix: None,
            },
            Check {
                name: "ar",
                ok: false,
                detail: "not found on PATH".to_string(),
                fix: Some("install binutils".to_string()),
            },
        ]
    }

    /// The JSON report must be a flat object carrying every check's full
    /// field set (`fix: None` becomes JSON `null`, never an omitted key) plus
    /// an accurate pass/fail tally — this is the shape CI tooling parses.
    #[test]
    fn json_report_carries_every_check_and_an_accurate_tally() {
        let checks = sample_checks();
        let rendered = build_json_report(&checks).render();

        assert!(rendered.contains("\"passed\":1"));
        assert!(rendered.contains("\"failed\":1"));
        assert!(rendered.contains("\"name\":\"clang\""));
        assert!(rendered.contains("\"ok\":true"));
        assert!(rendered.contains("\"name\":\"ar\""));
        assert!(rendered.contains("\"fix\":\"install binutils\""));
        assert!(rendered.contains("\"fix\":null"));
    }

    #[test]
    fn json_report_of_empty_checks_has_zero_tally() {
        let rendered = build_json_report(&[]).render();
        assert!(rendered.contains("\"passed\":0"));
        assert!(rendered.contains("\"failed\":0"));
        assert!(rendered.contains("\"checks\":[]"));
    }

    /// `doctor` stays project-agnostic by default: with no `cbld.toml`
    /// reachable (or none declaring a `toolchain` pin), no check is emitted
    /// at all — not a failing one.
    #[test]
    fn toolchain_check_is_absent_without_a_pinned_manifest() {
        // This crate's own repo root has no cbld.toml, so running the test
        // suite from anywhere under it must see no pin.
        assert!(check_toolchain_pin().is_none());
    }
}
