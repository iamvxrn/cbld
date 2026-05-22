//! Compilation database (`compile_commands.json`) generation.
//!
//! Every successful `cbld build` writes a clangd-compatible compilation
//! database to the project root, so IDEs and language servers (VS Code,
//! Neovim, CLion, ...) pick up cbld's exact standard version, warnings,
//! defines, and include paths with zero manual configuration. See
//! <https://clang.llvm.org/docs/JSONCompilationDatabase.html> for the format.

use std::path::{Path, PathBuf};

use crate::error::{IoPathExt, Result};
use crate::json::Json;

/// One translation unit's compile command, matching clangd's
/// `compile_commands.json` schema field-for-field.
#[derive(Debug, Clone)]
pub struct CompileCommandEntry {
    /// The absolute working directory the compiler was (would be) invoked
    /// from — cbld always runs clang with its own inherited cwd, so this is
    /// the same value for every entry in a given build.
    pub directory: PathBuf,
    /// The translation unit's source file, exactly as passed to clang.
    pub file: PathBuf,
    /// The full argument vector, compiler executable included
    /// (`clang`/`clang++` first, then every flag cbld generated for this
    /// unit — standard, optimization, warnings, includes, defines, `-o` and
    /// the object path, then the source path).
    pub arguments: Vec<String>,
}

impl CompileCommandEntry {
    fn to_json(&self) -> Json {
        Json::Object(vec![
            (
                "directory".to_string(),
                Json::str(self.directory.display().to_string()),
            ),
            (
                "file".to_string(),
                Json::str(self.file.display().to_string()),
            ),
            (
                "arguments".to_string(),
                Json::Array(
                    self.arguments
                        .iter()
                        .map(|a| Json::str(a.clone()))
                        .collect(),
                ),
            ),
        ])
    }
}

/// Write `compile_commands.json` to `root`, overwriting any file already
/// there. Pretty-printed since, unlike cbld's `--json` payloads, this file
/// is meant to be inspected and diffed by humans as well as tools.
pub fn write(root: &Path, entries: &[CompileCommandEntry]) -> Result<()> {
    let doc = Json::Array(entries.iter().map(CompileCommandEntry::to_json).collect());
    let path = root.join("compile_commands.json");
    std::fs::write(&path, doc.render_pretty()).path_ctx(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::Json;

    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cbld-compdb-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_produces_the_clangd_schema() {
        let dir = temp_dir("schema");
        let entries = vec![CompileCommandEntry {
            directory: PathBuf::from("/proj"),
            file: PathBuf::from("/proj/src/main.c"),
            arguments: vec![
                "clang".to_string(),
                "-c".to_string(),
                "-std=c17".to_string(),
                "src/main.c".to_string(),
            ],
        }];
        write(&dir, &entries).unwrap();

        let content = std::fs::read_to_string(dir.join("compile_commands.json")).unwrap();
        let parsed = Json::parse(&content).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0].get("directory").and_then(Json::as_str),
            Some("/proj")
        );
        assert_eq!(
            arr[0].get("file").and_then(Json::as_str),
            Some("/proj/src/main.c")
        );
        let args = arr[0].get("arguments").and_then(Json::as_array).unwrap();
        assert_eq!(args.len(), 4);
        assert_eq!(args[0].as_str(), Some("clang"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_overwrites_an_existing_file() {
        let dir = temp_dir("overwrite");
        let path = dir.join("compile_commands.json");
        std::fs::write(&path, "stale").unwrap();

        write(&dir, &[]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "[]");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
