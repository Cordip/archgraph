//! Imported third-party packages. GitNexus keeps an `IMPORTS` edge only when
//! an import resolves to a repository file and drops every other import
//! without a trace (docs/gitnexus-limitations.md), so which files use
//! OR-Tools or React is invisible to it. ArchGraph reads import statements in
//! Python and TypeScript/JavaScript itself and adds `IMPORTS` from the
//! importing file to the pseudo-path `package:<ecosystem>/<name>`.
//!
//! An import names a package only when it is clearly not local. Anything
//! that could be either goes to `PackageReport::ambiguous`, never an edge.
pub mod builtins;
pub mod python;
mod repository;
pub mod script;

use crate::{
    model::{
        package_id, AmbiguousImport, CodeEdge, Ecosystem, PackageImport, PackageReport, PackageUse,
        UnresolvedImport,
    },
    paths::provider_path,
    provider::typescript::is_script,
};
use anyhow::{Context, Result};
use repository::Repository;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
};

pub const KIND: &str = "IMPORTS";
pub const REASON: &str = "package-import";
/// `import type`, `export type … from`, or under `if TYPE_CHECKING:`.
pub const TYPE_REASON: &str = "package-type-import";

/// What a parser found in one file.
#[derive(Debug, Default)]
pub struct Facts {
    pub imports: Vec<Import>,
    /// Dynamic imports of a computed module: line and expression.
    pub unresolved: Vec<(usize, String)>,
}

/// The first argument of a call; comments between the parentheses are
/// named nodes too.
fn first_argument(arguments: tree_sitter::Node) -> Option<tree_sitter::Node> {
    let mut cursor = arguments.walk();
    let first = arguments
        .named_children(&mut cursor)
        .find(|argument| argument.kind() != "comment");
    first
}

#[derive(Debug)]
pub struct Import {
    pub line: usize,
    /// As written: `ortools.constraint_solver`, `..models`, `react-dom/client`.
    pub specifier: String,
    pub type_only: bool,
}

#[derive(Debug, Default)]
pub struct Extraction {
    pub edges: Vec<CodeEdge>,
    /// Packages without owners; the compiler assigns nodes.
    pub report: PackageReport,
    pub warnings: Vec<String>,
}

#[derive(Debug, PartialEq)]
enum Class {
    Package(Ecosystem, String),
    Local,
    Ambiguous(String),
}

/// A Python import is local when its top-level module sits beside the
/// importing file or above it (or in a `src/` there); a standard-library
/// module is no package; a top-level name the repository defines only
/// elsewhere may be either, so it is ambiguous.
fn classify_python(repository: &Repository, file: &str, specifier: &str) -> Class {
    if specifier.starts_with('.') {
        return Class::Local;
    }
    let top = specifier.split('.').next().unwrap_or(specifier);
    if top.is_empty() {
        return Class::Ambiguous("the module name is empty".into());
    }
    if repository.python_reachable(file, top) {
        return Class::Local;
    }
    if builtins::python_stdlib(top) {
        return Class::Local;
    }
    if let Some(directory) = repository.python_elsewhere(top) {
        let place = if directory.is_empty() {
            "the repository root".to_owned()
        } else {
            format!("`{directory}/`")
        };
        return Class::Ambiguous(format!("the repository has a Python module `{top}` in {place}, not beside the importing file or above it, so this may be that module or a package of the same name"));
    }
    Class::Package(Ecosystem::Python, top.to_owned())
}

/// The package a bare specifier names (`@scope/name` or `name`), or `None`
/// when it cannot be an npm package name (`@/x`, `~/x`, `$lib/x`), which
/// makes it a bundler or TypeScript path alias.
fn npm_name(specifier: &str) -> Option<&str> {
    let segment_ok = |segment: &str| {
        !segment.is_empty()
            && !segment.starts_with(['.', '_'])
            && segment
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_'))
    };
    let mut segments = specifier.splitn(3, '/');
    let first = segments.next()?;
    match first.strip_prefix('@') {
        Some(scope) => {
            let name = segments.next()?;
            (segment_ok(scope) && segment_ok(name))
                .then(|| &specifier[..first.len() + 1 + name.len()])
        }
        None => segment_ok(first).then_some(first),
    }
}

/// Whether the provider resolved `specifier`, imported by the file, to one
/// of `targets`: a target path ends with the specifier after its first
/// segment, as a TypeScript path alias maps `@app/utils/date` to
/// `src/app/utils/date.ts`.
fn provider_resolved(targets: &[String], specifier: &str) -> bool {
    let Some((_, rest)) = specifier.split_once('/') else {
        return false;
    };
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() {
        return false;
    }
    let stem = |path: &str| -> String {
        let name_start = path.rfind('/').map_or(0, |slash| slash + 1);
        let without = match path[name_start..].rfind('.') {
            Some(dot) if dot > 0 => &path[..name_start + dot],
            _ => path,
        };
        without.strip_suffix("/index").unwrap_or(without).to_owned()
    };
    let wanted = stem(rest);
    targets.iter().any(|target| {
        let target = stem(target);
        target == wanted || target.ends_with(&format!("/{wanted}"))
    })
}

/// A bare specifier is a package when a `package.json` beside the importing
/// file or above it declares it; a workspace package of the repository is
/// local, and so is an alias the provider resolved. An undeclared name may be
/// an alias the provider could not resolve or an undeclared package, so it
/// is ambiguous; an undeclared Node.js built-in is no package.
fn classify_script(
    repository: &Repository,
    targets: &[String],
    file: &str,
    specifier: &str,
) -> Class {
    if specifier.is_empty() {
        return Class::Ambiguous("the module specifier is empty".into());
    }
    if specifier.starts_with(['.', '/', '#']) {
        return Class::Local;
    }
    let bare = specifier.split(['?', '#']).next().unwrap_or(specifier);
    if let Some((scheme, _)) = bare.split_once(':') {
        if !scheme.contains('/') {
            return if scheme == "node" {
                Class::Local
            } else {
                Class::Ambiguous(format!(
                    "`{scheme}:` specifiers name no npm package (a bundler plugin or a URL)"
                ))
            };
        }
    }
    let Some(name) = npm_name(bare) else {
        return Class::Local;
    };
    if repository.workspace_package(name) {
        return Class::Local;
    }
    if repository.declared(file, name) {
        return Class::Package(Ecosystem::Npm, name.to_owned());
    }
    if provider_resolved(targets, bare) {
        return Class::Local;
    }
    if builtins::node_builtin(name) {
        return Class::Local;
    }
    Class::Ambiguous(format!("no package.json beside the importing file or above it declares `{name}`, and the code-graph provider did not resolve it to a repository file: a path alias or an undeclared package"))
}

pub fn extract(root: &Path, files: &[String], provider_edges: &[CodeEdge]) -> Result<Extraction> {
    let repository = Repository::read(root)?;
    let mut extraction = Extraction {
        warnings: repository.warnings.clone(),
        ..Extraction::default()
    };
    let mut resolved: HashMap<String, Vec<String>> = HashMap::new();
    for edge in provider_edges.iter().filter(|edge| edge.kind == KIND) {
        if let (Ok(from), Ok(to)) = (
            provider_path(root, &edge.from_file),
            provider_path(root, &edge.to_file),
        ) {
            resolved.entry(from).or_default().push(to);
        }
    }
    let mut packages: BTreeMap<String, (Ecosystem, String, BTreeSet<PackageImport>)> =
        BTreeMap::new();
    let mut edges: BTreeSet<(String, String, bool)> = BTreeSet::new();
    for path in files {
        let is_python = python::is_python(path);
        if !is_python && !is_script(path) {
            continue;
        }
        let source = std::fs::read_to_string(root.join(path))
            .with_context(|| format!("cannot read {path}"))?;
        let facts = if is_python {
            python::parse(path, &source)?
        } else {
            script::parse(path, &source)?
        };
        let targets = resolved.get(path).map(Vec::as_slice).unwrap_or_default();
        for import in facts.imports {
            let class = if is_python {
                classify_python(&repository, path, &import.specifier)
            } else {
                classify_script(&repository, targets, path, &import.specifier)
            };
            match class {
                Class::Local => {}
                Class::Ambiguous(reason) => extraction.report.ambiguous.push(AmbiguousImport {
                    file: path.clone(),
                    line: import.line,
                    specifier: import.specifier,
                    reason,
                }),
                Class::Package(ecosystem, name) => {
                    let id = package_id(ecosystem, &name);
                    edges.insert((path.clone(), id.clone(), import.type_only));
                    packages
                        .entry(id)
                        .or_insert_with(|| (ecosystem, name, BTreeSet::new()))
                        .2
                        .insert(PackageImport {
                            file: path.clone(),
                            line: import.line,
                            specifier: import.specifier,
                            type_only: import.type_only,
                            node: None,
                        });
                }
            }
        }
        extraction
            .report
            .unresolved
            .extend(
                facts
                    .unresolved
                    .into_iter()
                    .map(|(line, expression)| UnresolvedImport {
                        file: path.clone(),
                        line,
                        expression,
                    }),
            );
    }
    extraction.edges = edges
        .into_iter()
        .map(|(file, id, type_only)| CodeEdge {
            from_file: file,
            to_file: id,
            kind: KIND.into(),
            confidence: Some(1.0),
            reason: Some(if type_only { TYPE_REASON } else { REASON }.into()),
        })
        .collect();
    extraction.report.packages = packages
        .into_iter()
        .map(|(id, (ecosystem, name, imports))| PackageUse {
            id,
            name,
            ecosystem,
            node: None,
            imports: imports.into_iter().collect(),
        })
        .collect();
    extraction.report.ambiguous.sort();
    extraction.report.unresolved.sort();
    Ok(extraction)
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
    fn npm_names_of_bare_specifiers() {
        for (specifier, name) in [
            ("react", Some("react")),
            ("react-dom/client", Some("react-dom")),
            ("@tanstack/react-query", Some("@tanstack/react-query")),
            (
                "@tanstack/react-query/devtools",
                Some("@tanstack/react-query"),
            ),
            ("lodash.debounce", Some("lodash.debounce")),
            ("@/components/Map", None),
            ("~/utils", None),
            ("$lib/api", None),
            ("@scope", None),
        ] {
            assert_eq!(npm_name(specifier), name, "{specifier}");
        }
    }

    #[test]
    fn aliases_the_provider_resolved_are_local() {
        let targets = [
            "web/src/app/utils/date.ts".to_owned(),
            "web/src/ui/index.tsx".to_owned(),
        ];
        assert!(provider_resolved(&targets, "@app/utils/date"));
        assert!(provider_resolved(&targets, "src/ui"));
        assert!(!provider_resolved(&targets, "@app/utils/time"));
        assert!(!provider_resolved(&targets, "config"));
    }

    #[test]
    fn separates_packages_from_local_standard_and_ambiguous_imports() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "backend/planner/__init__.py", "");
        write(root, "backend/planner/core/models.py", "");
        write(
            root,
            "backend/planner/core/solver.py",
            "from __future__ import annotations\n\
             import numpy as np\n\
             from ortools.constraint_solver import pywrapcp\n\
             from ortools.constraint_solver import routing_enums_pb2\n\
             from planner.core.models import Plan\n\
             from .models import Plan\n\
             import dataclasses, os.path\n\
             import redis\n\
             from typing import TYPE_CHECKING\n\
             if TYPE_CHECKING:\n    import pandas\n",
        );
        write(
            root,
            "backend/tests/test_solver.py",
            "from ortools.sat.python import cp_model\nfrom tests.conftest import x\n",
        );
        write(root, "backend/tests/conftest.py", "");
        write(root, "tools/redis.py", "");
        write(
            root,
            "web/package.json",
            r#"{"name": "web", "dependencies": {"react": "19", "leaflet": "1", "events": "3"}, "devDependencies": {"@types/geojson": "7", "vite": "6"}, "workspaces": ["packages/*"]}"#,
        );
        write(
            root,
            "web/packages/ui/package.json",
            r#"{"name": "@acme/ui"}"#,
        );
        write(root, "web/packages/ui/index.ts", "export const ui = 1\n");
        write(
            root,
            "web/src/map.tsx",
            "import { useState } from 'react'\n\
             import L from 'leaflet'\n\
             import 'leaflet/dist/leaflet.css'\n\
             import type { Feature } from 'geojson'\n\
             import { ui } from '@acme/ui'\n\
             import { date } from '@app/utils/date'\n\
             import { config } from 'config'\n\
             import { readFile } from 'node:fs'\n\
             import path from 'path'\n\
             import { EventEmitter } from 'events'\n\
             import Icons from 'virtual:icons'\n\
             import { local } from './local'\n\
             const page = import(route)\n",
        );
        write(
            root,
            "web/vite.config.ts",
            "import { defineConfig } from 'vite'\n",
        );
        let provider = [CodeEdge {
            from_file: "web/src/map.tsx".into(),
            to_file: "web/src/app/utils/date.ts".into(),
            kind: "IMPORTS".into(),
            confidence: Some(1.0),
            reason: None,
        }];
        let files: Vec<String> = [
            "backend/planner/core/solver.py",
            "backend/tests/test_solver.py",
            "web/src/map.tsx",
            "web/vite.config.ts",
        ]
        .map(String::from)
        .to_vec();
        let extraction = extract(root, &files, &provider).unwrap();
        type Imported<'a> = (&'a str, Vec<(&'a str, usize, bool)>);
        let packages: Vec<Imported> = extraction
            .report
            .packages
            .iter()
            .map(|package| {
                (
                    package.id.as_str(),
                    package
                        .imports
                        .iter()
                        .map(|import| (import.file.as_str(), import.line, import.type_only))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            packages,
            [
                ("package:npm/events", vec![("web/src/map.tsx", 10, false)]),
                ("package:npm/geojson", vec![("web/src/map.tsx", 4, true)]),
                (
                    "package:npm/leaflet",
                    vec![("web/src/map.tsx", 2, false), ("web/src/map.tsx", 3, false)]
                ),
                ("package:npm/react", vec![("web/src/map.tsx", 1, false)]),
                ("package:npm/vite", vec![("web/vite.config.ts", 1, false)]),
                (
                    "package:python/numpy",
                    vec![("backend/planner/core/solver.py", 2, false)]
                ),
                (
                    "package:python/ortools",
                    vec![
                        ("backend/planner/core/solver.py", 3, false),
                        ("backend/planner/core/solver.py", 4, false),
                        ("backend/tests/test_solver.py", 1, false),
                    ]
                ),
                (
                    "package:python/pandas",
                    vec![("backend/planner/core/solver.py", 11, true)]
                ),
            ]
        );
        let ambiguous: Vec<(&str, &str)> = extraction
            .report
            .ambiguous
            .iter()
            .map(|import| (import.specifier.as_str(), import.reason.as_str()))
            .collect();
        assert_eq!(ambiguous.len(), 3, "{ambiguous:?}");
        assert_eq!(ambiguous[0].0, "redis");
        assert!(ambiguous[0].1.contains("`tools/`"), "{}", ambiguous[0].1);
        assert_eq!(ambiguous[1].0, "config");
        assert_eq!(ambiguous[2].0, "virtual:icons");
        assert_eq!(extraction.report.unresolved[0].expression, "import(route)");
        let edges: Vec<(&str, &str, &str)> = extraction
            .edges
            .iter()
            .map(|edge| {
                (
                    edge.from_file.as_str(),
                    edge.to_file.as_str(),
                    edge.reason.as_deref().unwrap(),
                )
            })
            .collect();
        assert!(edges.contains(&(
            "backend/planner/core/solver.py",
            "package:python/ortools",
            REASON
        )));
        assert!(edges.contains(&("web/src/map.tsx", "package:npm/geojson", TYPE_REASON)));
        assert_eq!(
            edges.len(),
            9,
            "one edge per file, package and reason: {edges:?}"
        );
    }
}
