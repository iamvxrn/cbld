//! Package-local C++20 module planning.
//!
//! The planner intentionally handles only standard module declarations and
//! imports found in the current package. Header units, `import std`, and
//! modules supplied by dependencies remain the compiler's responsibility.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::compiler::{CompileUnit, Language};
use crate::error::{CbldError, IoPathExt, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclarationKind {
    Interface,
    Implementation,
}

#[derive(Debug, Clone)]
struct SourceInfo {
    declaration: Option<(DeclarationKind, String)>,
    imports: Vec<String>,
}

/// Topological compilation waves. Every source index occurs exactly once.
#[derive(Debug, Clone, Default)]
pub struct ModuleGraph {
    pub has_modules: bool,
    pub waves: Vec<Vec<usize>>,
}

/// Parse module declarations, attach BMI flags, and produce dependency waves.
pub fn prepare(units: &mut [CompileUnit]) -> Result<ModuleGraph> {
    let mut infos = Vec::with_capacity(units.len());
    let mut has_modules = false;

    for unit in units.iter() {
        if unit.language != Language::Cpp {
            infos.push(SourceInfo {
                declaration: None,
                imports: Vec::new(),
            });
            continue;
        }
        let text = fs::read_to_string(&unit.source).path_ctx(&unit.source)?;
        let info = parse_source(&text);
        let is_module_extension = Language::is_module_source(&unit.source);
        if is_module_extension && info.declaration.is_none() {
            return Err(CbldError::LayoutViolation(format!(
                "module source '{}' has no module declaration",
                unit.source.display()
            )));
        }
        has_modules |=
            is_module_extension || info.declaration.is_some() || !info.imports.is_empty();
        infos.push(info);
    }

    if !has_modules {
        return Ok(ModuleGraph {
            has_modules: false,
            waves: vec![(0..units.len()).collect()],
        });
    }

    let mut producers = BTreeMap::<String, usize>::new();
    for (index, info) in infos.iter().enumerate() {
        if let Some((DeclarationKind::Interface, name)) = &info.declaration {
            if let Some(previous) = producers.insert(name.clone(), index) {
                return Err(CbldError::LayoutViolation(format!(
                    "duplicate C++ module producer '{}' in '{}' and '{}'",
                    name,
                    units[previous].source.display(),
                    units[index].source.display()
                )));
            }
        }
    }

    let mut module_files = BTreeMap::<String, PathBuf>::new();
    for (name, index) in &producers {
        let module_dir = units[*index]
            .object
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("modules");
        fs::create_dir_all(&module_dir).path_ctx(&module_dir)?;
        let path = module_dir.join(format!("{}.pcm", sanitize_module_name(name)));
        module_files.insert(name.clone(), path);
    }

    let mut dependencies = vec![BTreeSet::<usize>::new(); units.len()];
    for (index, info) in infos.iter().enumerate() {
        let mut imports = info.imports.clone();
        if let Some((DeclarationKind::Implementation, name)) = &info.declaration {
            // A module implementation unit implicitly consumes its primary
            // interface when that interface is part of this package.
            if let Some(primary) = name.split(':').next() {
                imports.push(primary.to_string());
            }
        }
        for import in imports {
            if let Some(&producer) = producers.get(&import) {
                if producer == index {
                    return Err(CbldError::LayoutViolation(format!(
                        "C++ module '{}' imports itself in '{}'",
                        import,
                        units[index].source.display()
                    )));
                }
                dependencies[index].insert(producer);
            }
        }
    }

    for (index, info) in infos.iter().enumerate() {
        let Some((kind, name)) = &info.declaration else {
            continue;
        };
        if *kind == DeclarationKind::Interface {
            if let Some(path) = module_files.get(name) {
                units[index]
                    .args
                    .push(format!("-fmodule-output={}", path.display()));
            }
        }
    }

    for (index, deps) in dependencies.iter().enumerate() {
        for producer in deps {
            let (name, path) = producers
                .iter()
                .find(|(_, source_index)| source_index == &producer)
                .and_then(|(name, _)| module_files.get(name).map(|path| (name, path)))
                .expect("every module producer has a BMI path");
            units[index]
                .args
                .push(format!("-fmodule-file={name}={}", path.display()));
        }
    }

    let waves = topological_waves(&dependencies, units)?;
    Ok(ModuleGraph { has_modules, waves })
}

fn topological_waves(
    dependencies: &[BTreeSet<usize>],
    units: &[CompileUnit],
) -> Result<Vec<Vec<usize>>> {
    let mut remaining: BTreeSet<usize> = (0..units.len()).collect();
    let mut waves = Vec::new();
    while !remaining.is_empty() {
        let wave: Vec<usize> = remaining
            .iter()
            .copied()
            .filter(|index| dependencies[*index].is_disjoint(&remaining))
            .collect();
        if wave.is_empty() {
            let cycle = remaining
                .iter()
                .map(|index| units[*index].source.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CbldError::LayoutViolation(format!(
                "cyclic C++ module imports involving: {cycle}"
            )));
        }
        for index in &wave {
            remaining.remove(index);
        }
        waves.push(wave);
    }
    Ok(waves)
}

fn sanitize_module_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn parse_source(text: &str) -> SourceInfo {
    let tokens = lex(text);
    let mut declaration = None;
    let mut imports = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index] == "export" && tokens.get(index + 1).map(String::as_str) == Some("module")
        {
            if let Some(name) = module_name(tokens.get(index + 2)) {
                declaration.get_or_insert((DeclarationKind::Interface, name));
            }
            index += 2;
        } else if tokens[index] == "module" {
            if let Some(name) = module_name(tokens.get(index + 1)) {
                declaration.get_or_insert((DeclarationKind::Implementation, name));
            }
        } else if tokens[index] == "import"
            && tokens.get(index + 1).is_some()
            && !tokens[index + 1].starts_with('<')
            && !tokens[index + 1].starts_with('"')
            && tokens[index + 1] != ";"
        {
            let import = tokens[index + 1].trim_end_matches(';');
            if !import.is_empty() && import != ":private" {
                let normalized = if import.starts_with(':') {
                    declaration
                        .as_ref()
                        .and_then(|(_, name)| name.split(':').next())
                        .map(|base| format!("{base}{import}"))
                        .unwrap_or_else(|| import.to_string())
                } else {
                    import.to_string()
                };
                imports.push(normalized);
            }
        }
        index += 1;
    }
    SourceInfo {
        declaration,
        imports,
    }
}

fn module_name(token: Option<&String>) -> Option<String> {
    let name = token?.trim_end_matches(';');
    if name.is_empty() || name.starts_with(':') {
        None
    } else {
        Some(name.to_string())
    }
}

/// Tokenize only the declaration vocabulary and discard comments/strings.
fn lex(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            }
            b'"' | b'\'' => {
                let quote = bytes[index];
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == quote {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            b'R' if bytes.get(index + 1) == Some(&b'"') => {
                if let Some(end) = raw_string_end(bytes, index) {
                    index = end;
                } else {
                    index += 1;
                }
            }
            byte if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b':' => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b':'))
                {
                    index += 1;
                }
                tokens.push(String::from_utf8_lossy(&bytes[start..index]).into_owned());
            }
            b'<' => {
                let start = index;
                index += 1;
                while index < bytes.len() && bytes[index] != b'>' {
                    index += 1;
                }
                if index < bytes.len() {
                    index += 1;
                }
                tokens.push(String::from_utf8_lossy(&bytes[start..index]).into_owned());
            }
            byte if byte.is_ascii_whitespace() => index += 1,
            byte => {
                if byte == b';' {
                    tokens.push(";".into());
                }
                index += 1;
            }
        }
    }
    tokens
}

fn raw_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let delimiter_start = start + 2;
    let open = (delimiter_start..bytes.len().min(delimiter_start + 17))
        .find(|&index| bytes[index] == b'(')?;
    let delimiter = &bytes[delimiter_start..open];
    let mut index = open + 1;
    while index < bytes.len() {
        if bytes[index] == b')'
            && bytes.get(index + 1..index + 1 + delimiter.len()) == Some(delimiter)
            && bytes.get(index + 1 + delimiter.len()) == Some(&b'"')
        {
            return Some(index + delimiter.len() + 2);
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::compiler::Compiler;
    use crate::manifest::{CProfile, CppProfile};

    fn unit(root: &Path, name: &str, text: &str) -> CompileUnit {
        let source = root.join(name);
        fs::write(&source, text).unwrap();
        let compiler = Compiler::new(
            CProfile::default(),
            CppProfile::default(),
            root,
            Vec::new(),
            &[],
            false,
            false,
            None,
            Vec::new(),
            false,
        );
        compiler
            .compile_unit(&source, &root.join(format!("{name}.o")))
            .unwrap()
    }

    #[test]
    fn orders_importers_after_interfaces_and_adds_bmi_flags() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut units = vec![
            unit(root, "main.cpp", "import math;\nint main() { return 0; }"),
            unit(
                root,
                "math.cppm",
                "export module math;\nexport int value = 1;",
            ),
        ];
        let graph = prepare(&mut units).unwrap();
        assert_eq!(graph.waves, vec![vec![1], vec![0]]);
        assert!(units[1]
            .args
            .iter()
            .any(|arg| arg.starts_with("-fmodule-output=")));
        assert!(units[0]
            .args
            .iter()
            .any(|arg| arg.starts_with("-fmodule-file=math=")));
    }

    #[test]
    fn ignores_module_words_in_comments_and_strings() {
        let info = parse_source(
            "// export module fake;\n/* export module also_fake; */\nconst char *s = \"import fake;\";\nexport module real;",
        );
        assert_eq!(
            info.declaration,
            Some((DeclarationKind::Interface, "real".into()))
        );
        assert!(info.imports.is_empty());
    }

    #[test]
    fn ignores_module_words_in_raw_strings() {
        let info = parse_source(
            r##"const char *s = R"tag(export module fake; // still text)tag";
                    export module real;"##,
        );
        assert_eq!(
            info.declaration,
            Some((DeclarationKind::Interface, "real".into()))
        );
    }

    #[test]
    fn rejects_module_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut units = vec![
            unit(root, "a.cppm", "export module a; import b;"),
            unit(root, "b.cppm", "export module b; import a;"),
        ];
        assert!(prepare(&mut units).is_err());
    }
}
