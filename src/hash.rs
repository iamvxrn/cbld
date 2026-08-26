//! Global build cache: deterministic package hashing plus on-disk storage of
//! prebuilt static archives under `~/.cbld/cache/prebuilt/{hash}/`.
//!
//! Hashing uses `std::hash::Hasher` (`DefaultHasher`, a SipHash variant) over
//! source content/mtimes, the resolved compiler flag fingerprint, and the
//! target OS/arch — no extra crate, matching cbld's zero-dependency
//! footprint (see website/docs/architecture.md).

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::error::{IoPathExt, Result};

/// Compute a deterministic cache key for one package build.
///
/// Folds in, per source file, its path, content, and modification time; the
/// package-wide compiler flag fingerprint (standard, optimization, warnings,
/// defines, ...); and the target OS/arch. Any change to any of these
/// produces a different key, so a hit only ever short-circuits a build that
/// would have produced byte-identical inputs.
pub fn package_key(sources: &[PathBuf], flags: &[String]) -> Result<String> {
    let mut hasher = DefaultHasher::new();
    std::env::consts::OS.hash(&mut hasher);
    std::env::consts::ARCH.hash(&mut hasher);

    // Sort a copy so key order doesn't depend on directory-walk order.
    let mut sorted: Vec<&PathBuf> = sources.iter().collect();
    sorted.sort();

    for src in sorted {
        src.hash(&mut hasher);
        let bytes = fs::read(src).path_ctx(src)?;
        bytes.hash(&mut hasher);
        if let Ok(modified) = fs::metadata(src).and_then(|m| m.modified()) {
            modified.hash(&mut hasher);
        }
    }
    flags.hash(&mut hasher);

    Ok(format!("{:016x}", hasher.finish()))
}

/// `~/.cbld/cache/prebuilt/{key}` — the directory a given cache key maps to.
fn prebuilt_dir(cbld_home: &Path, key: &str) -> PathBuf {
    cbld_home.join("cache").join("prebuilt").join(key)
}

/// Platform-appropriate static archive filename for a library named `name`.
fn archive_filename(name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{name}.lib")
    } else {
        format!("lib{name}.a")
    }
}

/// Look up a cached static archive for `key`/`name`. Returns its path only
/// when the file actually exists on disk.
pub fn lookup(cbld_home: &Path, key: &str, name: &str) -> Option<PathBuf> {
    let path = prebuilt_dir(cbld_home, key).join(archive_filename(name));
    path.is_file().then_some(path)
}

/// Store a freshly-built static archive in the global cache, keyed by `key`.
pub fn store(cbld_home: &Path, key: &str, name: &str, archive: &Path) -> Result<()> {
    let dir = prebuilt_dir(cbld_home, key);
    fs::create_dir_all(&dir).path_ctx(&dir)?;
    let dest = dir.join(archive_filename(name));
    fs::copy(archive, &dest).path_ctx(&dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn same_inputs_produce_the_same_key() {
        let dir = std::env::temp_dir().join(format!("cbld-hash-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = write_temp(&dir, "a.c", "int main(void) { return 0; }\n");

        let flags = vec!["-std=c17".to_string(), "-O0".to_string()];
        let key_a = package_key(std::slice::from_ref(&src), &flags).unwrap();
        let key_b = package_key(std::slice::from_ref(&src), &flags).unwrap();
        assert_eq!(key_a, key_b);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn changing_source_content_changes_the_key() {
        let dir = std::env::temp_dir().join(format!("cbld-hash-test2-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = write_temp(&dir, "a.c", "int main(void) { return 0; }\n");
        let flags = vec!["-std=c17".to_string()];
        let before = package_key(std::slice::from_ref(&src), &flags).unwrap();

        write_temp(&dir, "a.c", "int main(void) { return 1; }\n");
        let after = package_key(std::slice::from_ref(&src), &flags).unwrap();

        assert_ne!(before, after);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn changing_flags_changes_the_key() {
        let dir = std::env::temp_dir().join(format!("cbld-hash-test3-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = write_temp(&dir, "a.c", "int main(void) { return 0; }\n");

        let key_o0 = package_key(std::slice::from_ref(&src), &["-O0".to_string()]).unwrap();
        let key_o3 = package_key(std::slice::from_ref(&src), &["-O3".to_string()]).unwrap();
        assert_ne!(key_o0, key_o3);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lookup_misses_when_archive_absent_then_hits_after_store() {
        let home = std::env::temp_dir().join(format!("cbld-hash-home-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();

        let key = "deadbeefcafef00d";
        assert!(lookup(&home, key, "mylib").is_none());

        let archive_src = home.join("built.a");
        fs::write(&archive_src, b"not a real archive").unwrap();
        store(&home, key, "mylib", &archive_src).unwrap();

        let hit = lookup(&home, key, "mylib").expect("expected a cache hit after store");
        assert_eq!(fs::read(&hit).unwrap(), b"not a real archive");

        let _ = fs::remove_dir_all(&home);
    }
}
