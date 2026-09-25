use crate::{
    config::{is_within, overlaps, RuleConfig},
    model::{ArchitectureIr, PackageUse},
    projection::{self, EntryKind, Projection, ProjectionEdge},
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContext {
    pub projection: Projection,
    pub rules: Vec<RuleConfig>,
    pub observed_incoming: Vec<ProjectionEdge>,
    pub observed_outgoing: Vec<ProjectionEdge>,
    pub observed_internal: Vec<ProjectionEdge>,
    pub diagnostics: Vec<String>,
    pub agent_contract: Vec<String>,
    /// With `provider.packages`: packages imported from this subtree (with
    /// those imports) or owned by it (with all imports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packages: Option<Vec<PackageUse>>,
}

fn packages_here(ir: &ArchitectureIr, node: &str) -> Option<Vec<PackageUse>> {
    let report = ir.packages.as_ref()?;
    let within =
        |owner: &Option<String>| owner.as_deref().is_some_and(|owner| is_within(owner, node));
    Some(
        report
            .packages
            .iter()
            .filter_map(|package| {
                let mut package = package.clone();
                if !within(&package.node) {
                    package.imports.retain(|import| within(&import.node));
                }
                (!package.imports.is_empty()).then_some(package)
            })
            .collect(),
    )
}

pub fn build(ir: &ArchitectureIr, node: &str, limit: usize) -> Result<AgentContext> {
    let projection = projection::project(ir, node, limit)?;
    let outside: std::collections::BTreeSet<_> = projection
        .nodes
        .iter()
        .filter(|node| node.outside_focus)
        .map(|node| node.id.as_str())
        .collect();
    let mut incoming = Vec::new();
    let mut outgoing = Vec::new();
    let mut internal = Vec::new();
    for edge in &projection.edges {
        if edge.edge.origin != crate::model::EdgeOrigin::Observed {
            continue;
        }
        if outside.contains(edge.edge.from.as_str()) {
            incoming.push(edge.clone());
        } else if outside.contains(edge.edge.to.as_str()) {
            outgoing.push(edge.clone());
        } else {
            internal.push(edge.clone());
        }
    }
    let rules = ir
        .rules
        .iter()
        .filter(|rule| {
            rule.references()
                .iter()
                .any(|reference| overlaps(reference, node))
                || projection
                    .violations
                    .iter()
                    .any(|violation| violation.rule_id == rule.id())
        })
        .cloned()
        .collect();
    let agent_contract = vec![
        "Preserve existing behavior unless the task explicitly asks to change it.".into(),
        "Treat architecture.yaml as the desired architecture. Do not edit it unless the task explicitly requests an architecture change.".into(),
        "Use GitNexus context, impact or query for deeper symbol exploration; ArchGraph does not expose source contents.".into(),
        "After source edits, reindex with `gitnexus analyze --index-only` from the repository root.".into(),
        format!("Then run `archgraph check {node}` (or `archgraph check {node} --reindex` to do both)."),
        "Do not declare completion while check exits 2; exit 1 means verification failed, not a clean architecture.".into(),
        "Never run `archgraph baseline` unless explicitly asked: it accepts the current violations, like weakening a rule.".into(),
        crate::model::EVIDENCE_NOTICE.into(),
    ];
    Ok(AgentContext {
        projection,
        rules,
        observed_incoming: incoming,
        observed_outgoing: outgoing,
        observed_internal: internal,
        diagnostics: ir.diagnostics.warnings.clone(),
        agent_contract,
        packages: packages_here(ir, node),
    })
}

pub fn markdown(context: &AgentContext) -> String {
    let p = &context.projection;
    let mut out = format!("# Architecture context: {}\n\n", p.focus.id);
    let _ = writeln!(
        out,
        "**{}** — {} files in this subtree, {} with observed dependencies.\n",
        p.focus.title, p.focus.descendant_file_count, p.focus.observed_file_count
    );
    let _ = writeln!(
        out,
        "Purpose: {}\n",
        p.focus
            .description
            .as_deref()
            .unwrap_or("No description authored.")
    );
    out.push_str("## Children and files\n\n");
    for node in p.nodes.iter().filter(|node| !node.outside_focus) {
        let identity = projection::identity(node);
        let _ = writeln!(
            out,
            "- `{identity}` — {} — {}{}{}",
            node.title,
            projection::size(node),
            projection::observed_suffix(node),
            if node.violation_rule_ids.is_empty() {
                ""
            } else {
                " [VIOLATION]"
            }
        );
        if node.entry_kind == EntryKind::DirectFiles {
            for path in &p.focus.direct_files {
                let _ = writeln!(out, "  - `{path}`");
            }
        }
    }
    out.push_str("\n## Interfaces\n\n");
    if p.focus.interfaces.is_empty() {
        out.push_str("No interfaces authored for this node.\n");
    }
    for interface in &p.focus.interfaces {
        let _ = writeln!(
            out,
            "- {} — {} {}{}{}",
            interface.name,
            interface
                .direction
                .as_deref()
                .unwrap_or("unspecified direction"),
            interface.kind,
            interface
                .protocol
                .as_ref()
                .map(|p| format!("/{p}"))
                .unwrap_or_default(),
            interface
                .contract
                .as_ref()
                .map(|c| format!(": `{c}`"))
                .unwrap_or_default()
        );
        if let Some(description) = &interface.description {
            let _ = writeln!(out, "  {description}");
        }
    }
    for (heading, edges) in [
        ("Observed internal dependencies", &context.observed_internal),
        ("Observed incoming dependencies", &context.observed_incoming),
        ("Observed outgoing dependencies", &context.observed_outgoing),
    ] {
        let _ = writeln!(out, "\n## {heading}\n");
        if edges.is_empty() {
            out.push_str("No observed dependencies at this projection level.\n");
        }
        for projected in edges {
            append_edge(&mut out, p, projected);
        }
    }
    if let Some(packages) = &context.packages {
        out.push_str("\n## Imported packages\n\n");
        if packages.is_empty() {
            out.push_str("No imported package is used or owned here.\n");
        }
        for package in packages {
            const SHOWN: usize = 10;
            let places = package
                .imports
                .iter()
                .take(SHOWN)
                .map(|import| format!("`{}:{}`", import.file, import.line))
                .collect::<Vec<_>>()
                .join(", ");
            let more = package.imports.len().saturating_sub(SHOWN);
            let _ = writeln!(
                out,
                "- `{}` ({}, `{}`, node `{}`): {places}{}",
                package.name,
                package.ecosystem.title(),
                package.id,
                package.node.as_deref().unwrap_or("ambiguous"),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            );
        }
    }
    out.push_str("\n## Manual relationships (descriptive intent)\n\n");
    for edge in p
        .edges
        .iter()
        .filter(|e| e.edge.origin == crate::model::EdgeOrigin::Manual)
    {
        append_edge(&mut out, p, edge);
        for manual in &edge.edge.manual_edges {
            let _ = writeln!(
                out,
                "  - `{}`: {}{}",
                manual.id,
                manual.label.as_deref().unwrap_or(&manual.kind),
                manual
                    .description
                    .as_ref()
                    .map(|d| format!(" — {d}"))
                    .unwrap_or_default()
            );
        }
    }
    out.push_str("\n## Rules affecting this node\n\n");
    if context.rules.is_empty() {
        out.push_str("No authored rules affect this node.\n");
    }
    for rule in &context.rules {
        let _ = writeln!(out, "### `{}`\n", rule.id());
        // Serde-backed JSON exactly describes flags/types instead of a lossy paraphrase.
        if let Ok(value) = serde_json::to_string_pretty(rule) {
            let _ = writeln!(out, "```json\n{value}\n```\n");
        }
    }
    out.push_str("\n## Violations\n\n");
    if p.violations.is_empty() {
        out.push_str("No matching observed architecture violations.\n");
    }
    for violation in &p.violations {
        let _ = writeln!(
            out,
            "### `{}`\n\n{}\n",
            violation.rule_id, violation.message
        );
        if !violation.suggested_cuts.is_empty() {
            let _ = writeln!(
                out,
                "Layers, upper to lower: {}. Removing these upward dependencies breaks the cycle with the fewest observed changes:\n",
                violation.layer_order.join(" > ")
            );
            for cut in &violation.suggested_cuts {
                let _ = writeln!(out, "- `{}` → `{}` × {}", cut.from, cut.to, cut.count);
            }
            out.push_str("\nEvidence for the suggested cut:\n\n");
        }
        for edge in &violation.architecture_edges {
            let is_cut = violation
                .suggested_cuts
                .iter()
                .any(|cut| cut.from == edge.from && cut.to == edge.to);
            if !violation.suggested_cuts.is_empty() && !is_cut {
                continue;
            }
            let _ = writeln!(
                out,
                "- `{}` → `{}` [{}] × {}",
                edge.from, edge.to, edge.kind, edge.count
            );
            append_evidence(&mut out, &edge.evidence, edge.count);
        }
    }
    if !context.diagnostics.is_empty() {
        out.push_str("\n## Coverage diagnostics\n\n");
        for diagnostic in &context.diagnostics {
            let _ = writeln!(out, "- {diagnostic}");
        }
    }
    out.push_str("\n## Agent contract\n\n");
    for instruction in &context.agent_contract {
        let _ = writeln!(out, "- {instruction}");
    }
    out
}

fn append_edge(out: &mut String, projection: &Projection, projected: &ProjectionEdge) {
    let edge = &projected.edge;
    let _ = writeln!(
        out,
        "- `{}` → `{}` [{}] × {}{}{}",
        projection.endpoint_name(&edge.from),
        projection.endpoint_name(&edge.to),
        edge.kind,
        edge.count,
        if projected.violation_rule_ids.is_empty() {
            String::new()
        } else {
            format!(" [VIOLATION: {}]", projected.violation_rule_ids.join(", "))
        },
        if projected.suggested_cut_rule_ids.is_empty() {
            String::new()
        } else {
            format!(
                " [SUGGESTED CUT: {}]",
                projected.suggested_cut_rule_ids.join(", ")
            )
        }
    );
    if edge.origin == crate::model::EdgeOrigin::Observed {
        append_evidence(out, &edge.evidence, edge.count);
    }
}
fn append_evidence(out: &mut String, evidence: &[crate::model::EdgeEvidence], count: usize) {
    for item in evidence {
        let _ = writeln!(
            out,
            "  - `{}` → `{}` [{}]{}{}",
            item.from_file,
            item.to_file,
            item.kind,
            item.confidence
                .map(|c| format!(" confidence={c}"))
                .unwrap_or_default(),
            item.reason
                .as_ref()
                .map(|r| format!(" — {r}"))
                .unwrap_or_default()
        );
    }
    if evidence.len() < count {
        let _ = writeln!(
            out,
            "  - Showing {} of {count} observations (deterministic sample).",
            evidence.len()
        );
    }
}
