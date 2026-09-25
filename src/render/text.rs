use crate::{model::EdgeOrigin, projection::Projection};
use std::fmt::Write;

pub fn render(projection: &Projection) -> String {
    let mut out = format!("{} — {}\n", projection.focus.id, projection.focus.title);
    if let Some(description) = &projection.focus.description {
        let _ = writeln!(out, "{description}");
    }
    out.push_str("\nChildren / files\n");
    for node in &projection.nodes {
        let _ = writeln!(
            out,
            "  {} — {} file(s){}{}",
            node.title,
            node.file_count,
            if node.outside_focus {
                " [outside focus]"
            } else {
                ""
            },
            if node.violation_rule_ids.is_empty() {
                ""
            } else {
                " [VIOLATION]"
            }
        );
        let _ = writeln!(
            out,
            "    {}",
            node.file_path
                .as_deref()
                .or(node.architecture_id.as_deref())
                .unwrap_or(&node.id)
        );
    }
    out.push_str("\nProjected dependencies\n");
    for projected in &projection.edges {
        let edge = &projected.edge;
        let _ = writeln!(
            out,
            "  {} -> {}  {} × {}{}{}",
            edge.from,
            edge.to,
            edge.kind,
            edge.count,
            if edge.origin == EdgeOrigin::Manual {
                " [manual]"
            } else {
                " [observed]"
            },
            if projected.violation_rule_ids.is_empty() {
                ""
            } else {
                " [VIOLATION]"
            }
        );
    }
    let _ = writeln!(
        out,
        "\nViolations touching this view: {}",
        projection.violations.len()
    );
    for violation in &projection.violations {
        let _ = writeln!(out, "  [{}] {}", violation.rule_id, violation.message);
    }
    let _ = writeln!(out, "\n{}", projection.evidence_notice);
    out
}
