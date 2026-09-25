//! `archgraph packages`: which files and nodes import each third-party
//! package, whole or for one package.
use crate::model::{AmbiguousImport, PackageReport, PackageUse};
use anyhow::{bail, Result};
use std::{collections::BTreeSet, fmt::Write};

/// The report for `wanted` (a name such as `ortools`, `python/ortools` or a
/// package ID), or everything. The same name in two ecosystems selects both.
pub fn select(report: &PackageReport, wanted: Option<&str>) -> Result<PackageReport> {
    let Some(wanted) = wanted else {
        return Ok(report.clone());
    };
    let wanted = wanted.trim_start_matches(crate::model::PACKAGE_PREFIX);
    let packages: Vec<PackageUse> = report
        .packages
        .iter()
        .filter(|package| {
            package.name == wanted
                || format!("{}/{}", package.ecosystem.as_str(), package.name) == wanted
        })
        .cloned()
        .collect();
    // An import that may be this package is part of the answer.
    let ambiguous: Vec<AmbiguousImport> = report
        .ambiguous
        .iter()
        .filter(|import| {
            let specifier = import.specifier.as_str();
            specifier == wanted
                || specifier.split('.').next() == Some(wanted)
                || specifier
                    .strip_prefix(wanted)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .cloned()
        .collect();
    if packages.is_empty() && ambiguous.is_empty() {
        bail!("no imported package is named `{wanted}`; `archgraph packages` lists them all");
    }
    Ok(PackageReport {
        packages,
        ambiguous,
        unresolved: Vec::new(),
    })
}

fn files(package: &PackageUse) -> BTreeSet<&str> {
    package
        .imports
        .iter()
        .map(|import| import.file.as_str())
        .collect()
}

pub fn render(report: &PackageReport, wanted: Option<&str>) -> String {
    let mut out = String::new();
    if wanted.is_none() {
        let importers: BTreeSet<&str> = report.packages.iter().flat_map(files).collect();
        let _ = writeln!(
            out,
            "Packages: {} imported by {} file(s)",
            report.packages.len(),
            importers.len()
        );
    }
    let mut packages: Vec<&PackageUse> = report.packages.iter().collect();
    packages.sort_by(|a, b| (&a.name, a.ecosystem).cmp(&(&b.name, b.ecosystem)));
    for package in packages {
        let nodes: BTreeSet<&str> = package
            .imports
            .iter()
            .filter_map(|import| import.node.as_deref())
            .collect();
        let _ = writeln!(
            out,
            "\n{}  ({}, {}; node {})",
            package.name,
            package.ecosystem.title(),
            package.id,
            package.node.as_deref().unwrap_or("ambiguous")
        );
        let _ = writeln!(
            out,
            "  imported by {} file(s) in {}",
            files(package).len(),
            if nodes.is_empty() {
                "no architecture node".to_owned()
            } else {
                nodes.into_iter().collect::<Vec<_>>().join(", ")
            }
        );
        let places: Vec<String> = package
            .imports
            .iter()
            .map(|import| format!("{}:{}", import.file, import.line))
            .collect();
        let width = places.iter().map(String::len).max().unwrap_or(0);
        for (import, place) in package.imports.iter().zip(&places) {
            let _ = writeln!(
                out,
                "  {place:width$}  {}{}  {}",
                import.specifier,
                if import.type_only { " (type only)" } else { "" },
                import.node.as_deref().unwrap_or("(unassigned)")
            );
        }
    }
    if !report.ambiguous.is_empty() {
        let _ = writeln!(
            out,
            "\nImports that may name a repository module or a package ({}); not observed:",
            report.ambiguous.len()
        );
        for import in &report.ambiguous {
            let _ = writeln!(
                out,
                "  {}:{}  {}\n    {}",
                import.file, import.line, import.specifier, import.reason
            );
        }
    }
    if !report.unresolved.is_empty() {
        let _ = writeln!(
            out,
            "\nDynamic imports of a module computed at runtime ({}); not observed:",
            report.unresolved.len()
        );
        for import in &report.unresolved {
            let _ = writeln!(
                out,
                "  {}:{}  {}",
                import.file, import.line, import.expression
            );
        }
    }
    out
}
