//! CSS observations. GitNexus has no CSS grammar and drops `import
//! './styles.css'` (docs/gitnexus-limitations.md), so ArchGraph reads
//! stylesheets and the class names scripts use itself, and reports them as
//! ordinary `CodeEdge`s next to the GitNexus ones:
//!
//! - `IMPORTS` from a script to a stylesheet it imports, and from a stylesheet
//!   to one it `@import`s;
//! - `USES_CLASS` from a script to every repository stylesheet defining a
//!   class the script uses.
//!
//! Stylesheets are global: a class links to wherever it is defined.
pub mod script;
pub mod stylesheet;

use crate::{
    model::{
        ClassCertainty, ClassStatus, ClassUse, CodeEdge, CssClass, CssReport, DynamicClassUse,
        SourceLine, Stylesheet,
    },
    paths::normalize_relative,
};
use anyhow::{Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Relation kinds only this provider produces; GitNexus is not asked for them.
pub const EDGE_TYPES: [&str; 1] = ["USES_CLASS"];

const SCRIPT_EXTENSIONS: [&str; 8] = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

#[derive(Debug, Default)]
pub struct Extraction {
    pub edges: Vec<CodeEdge>,
    pub report: CssReport,
    pub warnings: Vec<String>,
}

fn is_script(path: &str) -> bool {
    SCRIPT_EXTENSIONS.iter().any(|ext| path.ends_with(ext)) && !path.ends_with(".d.ts")
}

fn edge(from: &str, to: &str, kind: &str, reason: &str, confidence: f64) -> CodeEdge {
    CodeEdge {
        from_file: from.into(),
        to_file: to.into(),
        kind: kind.into(),
        confidence: Some(confidence),
        reason: Some(reason.into()),
    }
}

fn read(root: &Path, path: &str) -> Result<String> {
    std::fs::read_to_string(root.join(path)).with_context(|| format!("cannot read {path}"))
}

fn directory(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

/// A package stylesheet (`leaflet/dist/leaflet.css`), found in the nearest
/// `node_modules` above the importing file, like a bundler would.
fn package_stylesheet(root: &Path, importer: &str, specifier: &str) -> Option<String> {
    let mut directory = directory(importer);
    loop {
        let candidate = if directory.is_empty() {
            format!("node_modules/{specifier}")
        } else {
            format!("{directory}/node_modules/{specifier}")
        };
        if root.join(&candidate).is_file() {
            return Some(candidate);
        }
        if directory.is_empty() {
            return None;
        }
        directory = self::directory(directory);
    }
}

#[derive(Default)]
struct Collected {
    /// Class name -> repository definitions.
    definitions: BTreeMap<String, BTreeSet<SourceLine>>,
    /// Classes defined by imported package stylesheets.
    external: BTreeSet<String>,
    uses: BTreeMap<String, BTreeSet<ClassUse>>,
    dynamic: BTreeSet<DynamicClassUse>,
    stylesheets: BTreeMap<String, Stylesheet>,
}

impl Collected {
    /// Parses a package stylesheet once, for its class definitions only.
    fn external_stylesheet(
        &mut self,
        root: &Path,
        importer: &str,
        specifier: &str,
        warnings: &mut Vec<String>,
    ) -> Result<()> {
        if self.stylesheets.contains_key(specifier) {
            return Ok(());
        }
        let Some(path) = package_stylesheet(root, importer, specifier) else {
            warnings.push(format!(
                "{importer}: stylesheet `{specifier}` not found in node_modules; its classes count as undefined (install dependencies)"
            ));
            return Ok(());
        };
        let facts = stylesheet::parse(&path, &read(root, &path)?)?;
        let names: BTreeSet<String> = facts.classes.into_iter().map(|(name, _)| name).collect();
        self.stylesheets.insert(
            specifier.to_owned(),
            Stylesheet {
                path: specifier.to_owned(),
                external: true,
                class_count: names.len(),
            },
        );
        self.external.extend(names);
        Ok(())
    }
}

/// Resolves an import specifier against the importing file. Relative
/// specifiers give a repository path; anything else is a package.
fn resolve(importer: &str, specifier: &str) -> Result<Option<String>> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return Ok(None);
    }
    normalize_relative(&format!("{}/{specifier}", directory(importer)))
        .map(Some)
        .with_context(|| format!("{importer}: cannot resolve `{specifier}`"))
}

/// `files` are the discovered repository files; stylesheets and scripts among
/// them are read from disk.
pub fn extract(root: &Path, files: &[String]) -> Result<Extraction> {
    let mut extraction = Extraction::default();
    let mut collected = Collected::default();
    let warnings = &mut extraction.warnings;
    for path in files.iter().filter(|path| path.ends_with(".css")) {
        let facts = stylesheet::parse(path, &read(root, path)?)?;
        warnings.extend(facts.warnings);
        let mut names = BTreeSet::new();
        for (name, line) in facts.classes {
            names.insert(name.clone());
            collected
                .definitions
                .entry(name)
                .or_default()
                .insert(SourceLine {
                    file: path.clone(),
                    line,
                });
        }
        collected.stylesheets.insert(
            path.clone(),
            Stylesheet {
                path: path.clone(),
                external: false,
                class_count: names.len(),
            },
        );
        for (specifier, _) in facts.imports {
            match resolve(path, &specifier)? {
                Some(target) => {
                    extraction
                        .edges
                        .push(edge(path, &target, "IMPORTS", "css-at-import", 1.0))
                }
                None => collected.external_stylesheet(root, path, &specifier, warnings)?,
            }
        }
    }
    for path in files.iter().filter(|path| is_script(path)) {
        let facts = script::parse(path, &read(root, path)?)?;
        warnings.extend(facts.warnings);
        for (specifier, _) in facts.stylesheet_imports {
            match resolve(path, &specifier)? {
                Some(target) => {
                    extraction
                        .edges
                        .push(edge(path, &target, "IMPORTS", "css-import", 1.0))
                }
                None => collected.external_stylesheet(root, path, &specifier, warnings)?,
            }
        }
        for (name, line, certainty) in facts.classes {
            collected.uses.entry(name).or_default().insert(ClassUse {
                file: path.clone(),
                line,
                certainty,
            });
        }
        collected
            .dynamic
            .extend(facts.dynamic.into_iter().map(|dynamic| DynamicClassUse {
                file: path.clone(),
                line: dynamic.line,
                expression: dynamic.expression,
                prefix: dynamic.prefix,
            }));
    }
    extraction.edges.extend(class_edges(&collected));
    extraction.report = report(collected);
    Ok(extraction)
}

fn class_edges(collected: &Collected) -> Vec<CodeEdge> {
    let mut edges = Vec::new();
    for (name, uses) in &collected.uses {
        let Some(definitions) = collected.definitions.get(name) else {
            continue;
        };
        let stylesheets: BTreeSet<&str> = definitions.iter().map(|d| d.file.as_str()).collect();
        for class_use in uses {
            let (reason, confidence) = if stylesheets.len() > 1 {
                ("classname-ambiguous", 0.5)
            } else {
                match class_use.certainty {
                    ClassCertainty::Literal => ("classname-literal", 1.0),
                    ClassCertainty::Expression => ("classname-expression", 0.8),
                    ClassCertainty::Markup => ("classname-markup", 0.8),
                }
            };
            for stylesheet in &stylesheets {
                if *stylesheet != class_use.file {
                    edges.push(edge(
                        &class_use.file,
                        stylesheet,
                        "USES_CLASS",
                        reason,
                        confidence,
                    ));
                }
            }
        }
    }
    edges
}

fn report(collected: Collected) -> CssReport {
    let Collected {
        definitions,
        external,
        mut uses,
        dynamic,
        stylesheets,
    } = collected;
    let prefixes: BTreeSet<&str> = dynamic
        .iter()
        .filter_map(|dynamic| dynamic.prefix.as_deref())
        .collect();
    let names: BTreeSet<String> = definitions.keys().chain(uses.keys()).cloned().collect();
    let classes = names
        .into_iter()
        .map(|name| {
            let definitions: Vec<SourceLine> = definitions
                .get(&name)
                .map(|lines| lines.iter().cloned().collect())
                .unwrap_or_default();
            let uses: Vec<ClassUse> = uses
                .remove(&name)
                .map(|uses| uses.into_iter().collect())
                .unwrap_or_default();
            let external = external.contains(&name);
            // A class a package stylesheet defines is the package's to apply
            // (e.g. Leaflet adds `leaflet-tooltip` at runtime); a repository
            // rule for it is an override, not dead code.
            let status = match (uses.is_empty(), definitions.is_empty() && !external) {
                (false, false) => ClassStatus::Used,
                (false, true) => ClassStatus::Undefined,
                (true, _) if external => ClassStatus::Used,
                (true, _) if prefixes.iter().any(|prefix| name.starts_with(prefix)) => {
                    ClassStatus::PossiblyDynamic
                }
                (true, _) => ClassStatus::Unused,
            };
            CssClass {
                name,
                status,
                definitions,
                external,
                uses,
            }
        })
        .collect();
    CssReport {
        stylesheets: stylesheets.into_values().collect(),
        classes,
        dynamic_uses: dynamic.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, content: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn links_scripts_to_stylesheets_and_classifies_classes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "src/styles.css", "@import './base.css';\n.panel { }\n.orphan { }\n.em-idle { }\n.dup { }\n.leaflet-tooltip { }\n");
        write(root, "src/base.css", ".dup { }\n");
        write(
            root,
            "node_modules/leaflet/dist/leaflet.css",
            ".leaflet-pane { }\n.leaflet-tooltip { }\n",
        );
        write(
            root,
            "src/App.tsx",
            "import './styles.css';\nimport 'leaflet/dist/leaflet.css';\nexport const App = () => <div className=\"panel leaflet-pane dup missing\"><i className={`em-${s}`} /></div>;\n",
        );
        let files: Vec<String> = ["src/App.tsx", "src/base.css", "src/styles.css"]
            .map(String::from)
            .to_vec();
        let extraction = extract(root, &files).unwrap();
        let mut edges: Vec<(&str, &str, &str, &str)> = extraction
            .edges
            .iter()
            .map(|e| {
                (
                    e.from_file.as_str(),
                    e.to_file.as_str(),
                    e.kind.as_str(),
                    e.reason.as_deref().unwrap(),
                )
            })
            .collect();
        edges.sort();
        assert_eq!(
            edges,
            [
                (
                    "src/App.tsx",
                    "src/base.css",
                    "USES_CLASS",
                    "classname-ambiguous"
                ),
                ("src/App.tsx", "src/styles.css", "IMPORTS", "css-import"),
                (
                    "src/App.tsx",
                    "src/styles.css",
                    "USES_CLASS",
                    "classname-ambiguous"
                ),
                (
                    "src/App.tsx",
                    "src/styles.css",
                    "USES_CLASS",
                    "classname-literal"
                ),
                ("src/styles.css", "src/base.css", "IMPORTS", "css-at-import"),
            ]
        );
        let status: BTreeMap<&str, ClassStatus> = extraction
            .report
            .classes
            .iter()
            .map(|class| (class.name.as_str(), class.status))
            .collect();
        assert_eq!(
            status,
            BTreeMap::from([
                ("dup", ClassStatus::Used),
                ("em-idle", ClassStatus::PossiblyDynamic),
                ("leaflet-pane", ClassStatus::Used),
                ("leaflet-tooltip", ClassStatus::Used),
                ("missing", ClassStatus::Undefined),
                ("orphan", ClassStatus::Unused),
                ("panel", ClassStatus::Used),
            ])
        );
        let external: Vec<&Stylesheet> = extraction
            .report
            .stylesheets
            .iter()
            .filter(|sheet| sheet.external)
            .collect();
        assert_eq!(external.len(), 1);
        assert_eq!(external[0].path, "leaflet/dist/leaflet.css");
        assert!(extraction.warnings.is_empty(), "{:?}", extraction.warnings);
    }

    #[test]
    fn a_missing_package_stylesheet_is_a_warning() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "src/main.tsx", "import 'pkg/theme.css';\n");
        let extraction = extract(temp.path(), &["src/main.tsx".to_owned()]).unwrap();
        assert!(extraction.warnings[0].contains("`pkg/theme.css` not found in node_modules"));
    }
}
