//! `archgraph diff`: what changed since a snapshot, as text.
use crate::snapshot::Diff;
use std::fmt::Write;

/// Beyond this many lines a section lists its first ones and a count.
const SECTION_LIMIT: usize = 50;

fn node(node: &Option<String>) -> &str {
    node.as_deref().unwrap_or("(unassigned)")
}

fn section<T>(out: &mut String, title: &str, items: &[T], line: impl Fn(&T) -> String) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{title} ({}):", items.len());
    for item in items.iter().take(SECTION_LIMIT) {
        let _ = writeln!(out, "  {}", line(item));
    }
    if items.len() > SECTION_LIMIT {
        let _ = writeln!(
            out,
            "  … and {} more (--json lists all)",
            items.len() - SECTION_LIMIT
        );
    }
}

pub fn render(diff: &Diff) -> String {
    let mut out = String::new();
    let commit = diff
        .commit
        .as_deref()
        .map(|commit| {
            format!(
                " (taken at {})",
                commit.chars().take(12).collect::<String>()
            )
        })
        .unwrap_or_default();
    let scope = diff
        .scope
        .as_deref()
        .map(|scope| format!(" in {scope}"))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "Changes since snapshot `{}`{commit}{scope}",
        diff.snapshot
    );
    if diff.is_empty() {
        let _ = writeln!(
            out,
            "\nNo changes: the same files, dependencies and violations."
        );
        return out;
    }
    let _ = writeln!(
        out,
        "  violations: +{} / -{} observation(s)\n  dependencies: +{} / -{}\n  files: +{} / -{}, {} moved, {} reassigned",
        diff.violations_appeared.len(),
        diff.violations_resolved.len(),
        diff.dependencies_added.len(),
        diff.dependencies_removed.len(),
        diff.files_added.len(),
        diff.files_removed.len(),
        diff.files_moved.len(),
        diff.files_reassigned.len(),
    );
    section(
        &mut out,
        "New violation observations",
        &diff.violations_appeared,
        |entry| {
            format!(
                "[{}] {} -> {} [{}]",
                entry.rule_id, entry.from_file, entry.to_file, entry.kind
            )
        },
    );
    section(
        &mut out,
        "Resolved violation observations",
        &diff.violations_resolved,
        |entry| {
            format!(
                "[{}] {} -> {} [{}]",
                entry.rule_id, entry.from_file, entry.to_file, entry.kind
            )
        },
    );
    section(
        &mut out,
        "Dependencies added",
        &diff.dependencies_added,
        |change| {
            format!(
                "{} -> {} [{}]  ({} -> {})",
                change.from_file, change.to_file, change.kind, change.from_node, change.to_node
            )
        },
    );
    section(
        &mut out,
        "Dependencies removed",
        &diff.dependencies_removed,
        |change| {
            format!(
                "{} -> {} [{}]  ({} -> {})",
                change.from_file, change.to_file, change.kind, change.from_node, change.to_node
            )
        },
    );
    section(&mut out, "Files moved", &diff.files_moved, |moved| {
        if moved.node_before == moved.node_after {
            format!(
                "{} -> {}  ({})",
                moved.from,
                moved.to,
                node(&moved.node_after)
            )
        } else {
            format!(
                "{} -> {}  ({} -> {})",
                moved.from,
                moved.to,
                node(&moved.node_before),
                node(&moved.node_after)
            )
        }
    });
    section(
        &mut out,
        "Files reassigned by the architecture",
        &diff.files_reassigned,
        |file| {
            format!(
                "{}  ({} -> {})",
                file.path,
                node(&file.node_before),
                node(&file.node_after)
            )
        },
    );
    section(&mut out, "Files added", &diff.files_added, |file| {
        format!("{}  ({})", file.path, node(&file.node))
    });
    section(&mut out, "Files removed", &diff.files_removed, |file| {
        format!("{}  ({})", file.path, node(&file.node))
    });
    out
}
