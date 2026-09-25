use crate::{model::EdgeOrigin, projection::Projection};
use std::{collections::BTreeMap, fmt::Write};

// Mermaid's own #NN; escape syntax, not HTML entities. Node identities are
// generated, never untrusted IDs interpolated into Mermaid grammar.
fn label(value: &str) -> String {
    let mut escaped = String::new();
    for c in value.chars() {
        if c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '.' || c == '-' || c == '/' {
            escaped.push(c);
        } else {
            let _ = write!(escaped, "#{};", c as u32);
        }
    }
    escaped
}

pub fn render(projection: &Projection) -> String {
    let mut out = String::from("flowchart LR\n");
    let ids: BTreeMap<&str, String> = projection
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), format!("n{index}")))
        .collect();
    for node in &projection.nodes {
        let mut title = node.title.clone();
        if node.outside_focus {
            title.push_str(" (outside)");
        }
        if !node.violation_rule_ids.is_empty() {
            title.push_str(" [VIOLATION]");
        }
        let _ = writeln!(out, "  {}[\"{}\"]", ids[node.id.as_str()], label(&title));
    }
    for projected in &projection.edges {
        let edge = &projected.edge;
        let mut text = format!("{} × {}", edge.kind, edge.count);
        if edge.origin == EdgeOrigin::Manual {
            text.push_str(" manual");
        }
        if !projected.violation_rule_ids.is_empty() {
            text.push_str(" VIOLATION");
        }
        let arrow = if edge.origin == EdgeOrigin::Manual {
            "-.->"
        } else {
            "-->"
        };
        let _ = writeln!(
            out,
            "  {} {}|\"{}\"| {}",
            ids[edge.from.as_str()],
            arrow,
            label(&text),
            ids[edge.to.as_str()]
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hostile_labels_cannot_break_mermaid_grammar() {
        let escaped = label("\"] --> injected[\"<script>\n");
        assert!(!escaped.contains('"'));
        assert!(!escaped.contains('['));
        assert!(!escaped.contains('\n'));
        assert!(!escaped.contains('<'));
    }
}
