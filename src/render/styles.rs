//! `archgraph styles`: the class report, whole or for one node's subtree.
use crate::{
    config::is_within,
    model::{ArchitectureIr, ClassStatus, CssClass, CssReport, SourceLine},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write,
};

struct Owners<'a>(HashMap<&'a str, &'a str>);

impl<'a> Owners<'a> {
    fn new(ir: &'a ArchitectureIr) -> Self {
        Self(
            ir.files
                .iter()
                .filter_map(|file| Some((file.path.as_str(), file.node.as_deref()?)))
                .collect(),
        )
    }
    fn of(&self, file: &str) -> Option<&'a str> {
        self.0.get(file).copied()
    }
    fn within(&self, file: &str, node: Option<&str>) -> bool {
        node.is_none_or(|node| self.of(file).is_some_and(|owner| is_within(owner, node)))
    }
}

/// The part of the report that touches `node`: classes defined or used in
/// its subtree (with all their uses, so outside users stay visible),
/// stylesheets in it, and dynamic uses in it.
pub fn select(ir: &ArchitectureIr, report: &CssReport, node: Option<&str>) -> CssReport {
    let owners = Owners::new(ir);
    CssReport {
        stylesheets: report
            .stylesheets
            .iter()
            .filter(|sheet| sheet.external || owners.within(&sheet.path, node))
            .cloned()
            .collect(),
        classes: report
            .classes
            .iter()
            .filter(|class| {
                class
                    .definitions
                    .iter()
                    .any(|d| owners.within(&d.file, node))
                    || class.uses.iter().any(|u| owners.within(&u.file, node))
            })
            .cloned()
            .collect(),
        dynamic_uses: report
            .dynamic_uses
            .iter()
            .filter(|dynamic| owners.within(&dynamic.file, node))
            .cloned()
            .collect(),
    }
}

/// Classes used from more than one architecture node, with those nodes.
pub fn shared<'a>(ir: &'a ArchitectureIr, report: &'a CssReport) -> Vec<(&'a str, Vec<&'a str>)> {
    let owners = Owners::new(ir);
    report
        .classes
        .iter()
        .filter_map(|class| {
            let nodes: BTreeSet<&str> = class
                .uses
                .iter()
                .filter_map(|u| owners.of(&u.file))
                .collect();
            (nodes.len() > 1).then(|| (class.name.as_str(), nodes.into_iter().collect()))
        })
        .collect()
}

fn location(line: &SourceLine) -> String {
    format!("{}:{}", line.file, line.line)
}

fn section<'a>(
    out: &mut String,
    title: &str,
    classes: impl Iterator<Item = &'a CssClass>,
    detail: impl Fn(&CssClass) -> String,
) {
    let classes: Vec<&CssClass> = classes.collect();
    if classes.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{title} ({}):", classes.len());
    let width = classes
        .iter()
        .map(|class| class.name.len())
        .max()
        .unwrap_or(0)
        + 1;
    for class in classes {
        let _ = writeln!(out, "  .{:width$} {}", class.name, detail(class));
    }
}

pub fn render(ir: &ArchitectureIr, report: &CssReport, node: Option<&str>) -> String {
    let mut out = String::new();
    let scope = node.map_or(String::new(), |node| format!(" touching `{node}`"));
    let _ = writeln!(out, "Stylesheets{scope}:");
    for sheet in &report.stylesheets {
        let origin = if sheet.external { " (package)" } else { "" };
        let _ = writeln!(
            out,
            "  {}{origin}: {} classes",
            sheet.path, sheet.class_count
        );
    }
    let mut counts: BTreeMap<ClassStatus, usize> = BTreeMap::new();
    for class in &report.classes {
        *counts.entry(class.status).or_default() += 1;
    }
    let count = |status| counts.get(&status).copied().unwrap_or(0);
    let _ = writeln!(
        out,
        "Classes: {} used, {} undefined, {} unused, {} possibly used dynamically",
        count(ClassStatus::Used),
        count(ClassStatus::Undefined),
        count(ClassStatus::Unused),
        count(ClassStatus::PossiblyDynamic)
    );
    let of = |status| move |class: &&CssClass| class.status == status;
    let uses = |class: &CssClass| {
        class
            .uses
            .iter()
            .map(|u| format!("{}:{}", u.file, u.line))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let definitions = |class: &CssClass| {
        class
            .definitions
            .iter()
            .map(location)
            .collect::<Vec<_>>()
            .join(", ")
    };
    section(
        &mut out,
        "Undefined: used, but no stylesheet defines them",
        report.classes.iter().filter(of(ClassStatus::Undefined)),
        uses,
    );
    section(
        &mut out,
        "Unused: defined, never used",
        report.classes.iter().filter(of(ClassStatus::Unused)),
        definitions,
    );
    let prefixed: Vec<(&str, String)> = report
        .dynamic_uses
        .iter()
        .filter_map(|d| Some((d.prefix.as_deref()?, format!("{}:{}", d.file, d.line))))
        .collect();
    section(
        &mut out,
        "Possibly used dynamically: never used literally, but a dynamic class has their prefix",
        report
            .classes
            .iter()
            .filter(of(ClassStatus::PossiblyDynamic)),
        |class| {
            let sources: Vec<String> = prefixed
                .iter()
                .filter(|(prefix, _)| class.name.starts_with(prefix))
                .map(|(prefix, at)| format!("`{prefix}…` at {at}"))
                .collect();
            format!("{} ({})", definitions(class), sources.join(", "))
        },
    );
    let unresolved: Vec<_> = report
        .dynamic_uses
        .iter()
        .filter(|d| d.prefix.is_none())
        .collect();
    if !unresolved.is_empty() {
        let _ = writeln!(
            out,
            "\nUnresolved class expressions ({}); any class may come from them:",
            unresolved.len()
        );
        for dynamic in unresolved {
            let _ = writeln!(
                out,
                "  {}:{}  {}",
                dynamic.file, dynamic.line, dynamic.expression
            );
        }
    }
    let shared = shared(ir, report);
    if !shared.is_empty() {
        let _ = writeln!(
            out,
            "\nShared: used from more than one architecture node ({}):",
            shared.len()
        );
        let width = shared.iter().map(|(name, _)| name.len()).max().unwrap_or(0) + 1;
        for (name, nodes) in shared {
            let _ = writeln!(out, "  .{name:width$} {}", nodes.join(", "));
        }
    }
    out
}
