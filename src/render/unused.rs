//! `archgraph unused`: mapped files that no observed code uses, grouped by
//! node, and nodes nothing outside their subtree uses. Candidates for dead
//! code or undeclared entry points, never proof.
use crate::{
    config::is_within,
    model::{ArchitectureIr, CompiledNode, FileUsage, USAGE_NOTICE},
};
use serde::Serialize;
use std::{collections::BTreeMap, fmt::Write};

#[derive(Debug, Serialize)]
pub struct UnusedReport {
    pub node: Option<String>,
    /// Files with no observed users, excluding entry points and unindexed files.
    pub files: Vec<UnusedFile>,
    /// Topmost nodes in scope with no observed user outside their subtree,
    /// no entry point and some indexed files.
    pub nodes_without_outside_users: Vec<UnusedNode>,
    pub entry_points: Vec<String>,
    /// Not in the provider's index: whether anything uses them is unknown.
    pub not_indexed: Vec<String>,
    pub mapped_file_count: usize,
    pub observed_file_count: usize,
    /// No `project.entry_points` are declared at all.
    pub no_entry_points_declared: bool,
    pub notice: String,
}

#[derive(Debug, Serialize)]
pub struct UnusedFile {
    pub path: String,
    pub node: String,
}

#[derive(Debug, Serialize)]
pub struct UnusedNode {
    pub id: String,
    pub title: String,
    pub file_count: usize,
}

/// The report for the subtree of `node`, or for every mapped file.
pub fn select(ir: &ArchitectureIr, node: Option<&str>) -> UnusedReport {
    let within = |owner: &str| node.is_none_or(|node| is_within(owner, node));
    let mut files = Vec::new();
    let mut entry_points = Vec::new();
    let mut not_indexed = Vec::new();
    let mut mapped_file_count = 0;
    for file in &ir.files {
        let (Some(owner), Some(usage)) = (&file.node, file.usage) else {
            continue;
        };
        if !within(owner) {
            continue;
        }
        mapped_file_count += 1;
        match usage {
            FileUsage::NoObservedUsers => files.push(UnusedFile {
                path: file.path.clone(),
                node: owner.clone(),
            }),
            FileUsage::EntryPoint => entry_points.push(file.path.clone()),
            FileUsage::NotIndexed => not_indexed.push(file.path.clone()),
            FileUsage::Used => {}
        }
    }
    let observed_file_count = match node {
        Some(id) => ir.nodes.get(id).map_or(0, |node| node.observed_file_count),
        None => ir
            .nodes
            .values()
            .filter(|node| node.parent.is_none())
            .map(|node| node.observed_file_count)
            .sum(),
    };
    let mut nodes: Vec<&CompiledNode> = Vec::new();
    for candidate in ir.nodes.values() {
        if within(&candidate.id)
            && candidate.no_outside_users()
            && !nodes
                .iter()
                .any(|listed| is_within(&candidate.id, &listed.id))
        {
            nodes.push(candidate);
        }
    }
    UnusedReport {
        node: node.map(str::to_owned),
        files,
        nodes_without_outside_users: nodes
            .into_iter()
            .map(|node| UnusedNode {
                id: node.id.clone(),
                title: node.title.clone(),
                file_count: node.descendant_file_count,
            })
            .collect(),
        entry_points,
        not_indexed,
        mapped_file_count,
        observed_file_count,
        no_entry_points_declared: ir.project.entry_points.is_empty(),
        notice: USAGE_NOTICE.into(),
    }
}

pub fn render(ir: &ArchitectureIr, report: &UnusedReport) -> String {
    let mut out = String::new();
    let scope = report
        .node
        .as_deref()
        .map(|node| format!(" under `{node}`"))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "Files with no observed users: {} of {} mapped file(s){scope}",
        report.files.len(),
        report.mapped_file_count
    );
    let mut by_node: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for file in &report.files {
        by_node
            .entry(file.node.as_str())
            .or_default()
            .push(file.path.as_str());
    }
    for (node, paths) in by_node {
        let title = ir.nodes.get(node).map_or("", |node| node.title.as_str());
        let _ = writeln!(out, "\n{node}  ({title})");
        for path in paths {
            let _ = writeln!(out, "  {path}");
        }
    }
    if !report.nodes_without_outside_users.is_empty() {
        let _ = writeln!(
            out,
            "\nNodes that nothing outside them uses, with no entry point declared ({}):",
            report.nodes_without_outside_users.len()
        );
        for node in &report.nodes_without_outside_users {
            let _ = writeln!(
                out,
                "  {}  ({}, {} file(s))",
                node.id, node.title, node.file_count
            );
        }
    }
    let _ = writeln!(
        out,
        "\nDeclared entry points (project.entry_points): {} file(s)",
        report.entry_points.len()
    );
    if !report.not_indexed.is_empty() {
        let _ = writeln!(
            out,
            "Not in the code-graph index, usage unknown: {} file(s), e.g. {}",
            report.not_indexed.len(),
            report.not_indexed[0]
        );
    }
    if report.mapped_file_count > 0 {
        let _ = writeln!(
            out,
            "Coverage: {} of {} file(s) ({:.0}%) have any observed dependency; where it is low, most files above are unseen, not unused.",
            report.observed_file_count,
            report.mapped_file_count,
            100.0 * report.observed_file_count as f64 / report.mapped_file_count as f64
        );
    }
    if report.no_entry_points_declared {
        out.push_str("\nNo entry points are declared. Files that a tool, runtime or test runner loads by name (vite.config.ts, main.tsx, test_*.py, a module uvicorn starts) are listed above until you add them to project.entry_points.\n");
    }
    let _ = writeln!(out, "\n{}", report.notice);
    out
}
