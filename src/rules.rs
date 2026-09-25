//! Rules operate only on observed edges. Manual architectural intent is never
//! silently treated as source-code evidence.
use crate::{
    config::{immediate_child, is_within, RuleConfig},
    model::*,
};
use std::collections::{BTreeMap, BTreeSet};

fn in_scope(node: &str, scope: &str, descendants: bool) -> bool {
    node == scope || (descendants && is_within(node, scope))
}

pub fn violation_touches(violation: &Violation, node: &str) -> bool {
    violation
        .affected_nodes
        .iter()
        .any(|affected| is_within(affected, node))
}

pub fn evaluate(rules: &[RuleConfig], edges: &[ResolvedEdge], limit: usize) -> Vec<Violation> {
    let mut violations = Vec::new();
    for rule in rules {
        match rule {
            RuleConfig::DenyDependency {
                from,
                to,
                edge_types,
                include_descendants,
                ..
            } => {
                let forbidden = edges.iter().filter(|edge| {
                    edge_types.contains(&edge.kind)
                        && in_scope(&edge.from, from, *include_descendants)
                        && in_scope(&edge.to, to, *include_descendants)
                });
                violations.extend(dependency_violations(rule, forbidden, limit));
            }
            RuleConfig::AllowOnly {
                from,
                to,
                edge_types,
                include_descendants,
                ..
            } => {
                let forbidden = edges.iter().filter(|edge| {
                    edge_types.contains(&edge.kind)
                        && in_scope(&edge.from, from, *include_descendants)
                        && !is_within(&edge.to, from)
                        && !to
                            .iter()
                            .any(|target| in_scope(&edge.to, target, *include_descendants))
                });
                violations.extend(dependency_violations(rule, forbidden, limit));
            }
            RuleConfig::NoCycles {
                within, edge_types, ..
            } => {
                let mut projected = Vec::new();
                let mut original_nodes: BTreeMap<(String, String, String), BTreeSet<String>> =
                    BTreeMap::new();
                for edge in edges.iter().filter(|edge| edge_types.contains(&edge.kind)) {
                    let (Some(from), Some(to)) = (
                        immediate_child(&edge.from, within),
                        immediate_child(&edge.to, within),
                    ) else {
                        continue;
                    };
                    if from == to {
                        continue;
                    }
                    original_nodes
                        .entry((from.into(), to.into(), edge.kind.clone()))
                        .or_default()
                        .extend([edge.from.clone(), edge.to.clone()]);
                    projected.push(ResolvedEdge {
                        from: from.into(),
                        to: to.into(),
                        kind: edge.kind.clone(),
                        evidence: edge.evidence.clone(),
                    });
                }
                let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for edge in &projected {
                    graph
                        .entry(edge.from.clone())
                        .or_default()
                        .push(edge.to.clone());
                    graph.entry(edge.to.clone()).or_default();
                }
                for neighbors in graph.values_mut() {
                    neighbors.sort();
                    neighbors.dedup();
                }
                let components = strongly_connected_components(&graph);
                let component_of: BTreeMap<&str, usize> = components
                    .iter()
                    .enumerate()
                    .filter(|(_, component)| component.len() >= 2)
                    .flat_map(|(index, component)| {
                        component.iter().map(move |node| (node.as_str(), index))
                    })
                    .collect();
                let mut groups: BTreeMap<usize, Vec<&ResolvedEdge>> = BTreeMap::new();
                for edge in &projected {
                    if let (Some(&a), Some(&b)) = (
                        component_of.get(edge.from.as_str()),
                        component_of.get(edge.to.as_str()),
                    ) {
                        if a == b {
                            groups.entry(a).or_default().push(edge);
                        }
                    }
                }
                for (index, participating) in groups {
                    let architecture_edges =
                        aggregate_observed(participating.iter().copied(), limit);
                    let mut evidence = EvidenceAccumulator::default();
                    let mut affected_nodes = BTreeSet::new();
                    for edge in participating {
                        evidence.add(&edge.evidence, limit);
                        if let Some(owners) = original_nodes.get(&(
                            edge.from.clone(),
                            edge.to.clone(),
                            edge.kind.clone(),
                        )) {
                            affected_nodes.extend(owners.iter().cloned());
                        }
                    }
                    violations.push(Violation {
                        rule_id: rule.id().into(), kind: rule.kind().into(), from: Some(within.clone()), to: None,
                        edge_kind: if edge_types.len() == 1 { Some(edge_types[0].clone()) } else { None },
                        nodes: components[index].clone(), affected_nodes: affected_nodes.into_iter().collect(), count: evidence.count,
                        message: format!("cycle among immediate children of `{within}`: {}; {} observed dependencies participate (SCC, not an ordered cycle path)", components[index].join(", "), evidence.count),
                        evidence: evidence.evidence, architecture_edges,
                    });
                }
            }
        }
    }
    violations.sort_by(|a, b| {
        (&a.rule_id, &a.nodes, &a.from, &a.to, &a.edge_kind).cmp(&(
            &b.rule_id,
            &b.nodes,
            &b.from,
            &b.to,
            &b.edge_kind,
        ))
    });
    violations
}

fn dependency_violations<'a>(
    rule: &RuleConfig,
    edges: impl IntoIterator<Item = &'a ResolvedEdge>,
    limit: usize,
) -> Vec<Violation> {
    aggregate_observed(edges, limit)
        .into_iter()
        .map(|edge| {
            let nodes: BTreeSet<_> = [edge.from.clone(), edge.to.clone()].into_iter().collect();
            let nodes: Vec<String> = nodes.into_iter().collect();
            Violation {
                rule_id: rule.id().into(),
                kind: rule.kind().into(),
                from: Some(edge.from.clone()),
                to: Some(edge.to.clone()),
                edge_kind: Some(edge.kind.clone()),
                affected_nodes: nodes.clone(),
                nodes,
                count: edge.count,
                message: format!(
                    "{} -> {} [{}] × {} violates {} `{}`",
                    edge.from,
                    edge.to,
                    edge.kind,
                    edge.count,
                    rule.kind(),
                    rule.id()
                ),
                evidence: edge.evidence.clone(),
                architecture_edges: vec![edge],
            }
        })
        .collect()
}

/// Iterative Kosaraju: O(V+E) traversal, no call-stack depth limit. Sorted input
/// and output make SCC reporting deterministic without claiming an SCC is a path.
pub fn strongly_connected_components(graph: &BTreeMap<String, Vec<String>>) -> Vec<Vec<String>> {
    let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (from, targets) in graph {
        reverse.entry(from.clone()).or_default();
        for target in targets {
            reverse
                .entry(target.clone())
                .or_default()
                .push(from.clone());
        }
    }
    let mut visited = BTreeSet::new();
    let mut finish = Vec::new();
    for start in reverse.keys() {
        if visited.contains(start) {
            continue;
        }
        let mut stack = vec![(start.clone(), false)];
        while let Some((node, expanded)) = stack.pop() {
            if expanded {
                finish.push(node);
                continue;
            }
            if !visited.insert(node.clone()) {
                continue;
            }
            stack.push((node.clone(), true));
            if let Some(neighbors) = graph.get(&node) {
                for next in neighbors.iter().rev() {
                    if !visited.contains(next) {
                        stack.push((next.clone(), false));
                    }
                }
            }
        }
    }
    visited.clear();
    let mut components = Vec::new();
    for start in finish.into_iter().rev() {
        if !visited.insert(start.clone()) {
            continue;
        }
        let mut stack = vec![start];
        let mut component = Vec::new();
        while let Some(node) = stack.pop() {
            if let Some(neighbors) = reverse.get(&node) {
                for next in neighbors {
                    if visited.insert(next.clone()) {
                        stack.push(next.clone());
                    }
                }
            }
            component.push(node);
        }
        component.sort();
        components.push(component);
    }
    components.sort();
    components
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edge(from: &str, to: &str) -> ResolvedEdge {
        ResolvedEdge {
            from: from.into(),
            to: to.into(),
            kind: "IMPORTS".into(),
            evidence: EdgeEvidence {
                from_file: format!("{from}.rs"),
                to_file: format!("{to}.rs"),
                kind: "IMPORTS".into(),
                confidence: None,
                reason: None,
            },
        }
    }
    #[test]
    fn deny_dependency_subtrees_and_exact_mode() {
        let edges = vec![edge("app.a.child", "app.b"), edge("app.aa", "app.b")];
        let mut rule = RuleConfig::DenyDependency {
            id: "deny".into(),
            from: "app.a".into(),
            to: "app.b".into(),
            edge_types: vec!["IMPORTS".into()],
            include_descendants: true,
        };
        let found = evaluate(&[rule.clone()], &edges, 20);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].count, 1);
        if let RuleConfig::DenyDependency {
            include_descendants,
            ..
        } = &mut rule
        {
            *include_descendants = false;
        }
        assert!(evaluate(&[rule], &edges, 20).is_empty());
    }
    #[test]
    fn allow_only_permits_internal_and_allowlisted_dependencies() {
        let rule = RuleConfig::AllowOnly {
            id: "allow".into(),
            from: "app.a".into(),
            to: vec!["app.shared".into()],
            edge_types: vec!["IMPORTS".into()],
            include_descendants: true,
        };
        let edges = vec![
            edge("app.a.x", "app.a.y"),
            edge("app.a.x", "app.shared.x"),
            edge("app.a.x", "app.sharedish"),
            edge("app.b", "app.c"),
        ];
        let found = evaluate(&[rule], &edges, 20);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].to.as_deref(), Some("app.sharedish"));
    }
    #[test]
    fn cycles_are_sccs_at_immediate_child_level_with_all_edge_evidence() {
        let rule = RuleConfig::NoCycles {
            id: "cycles".into(),
            within: "app".into(),
            edge_types: vec!["IMPORTS".into()],
        };
        let edges = vec![
            edge("app.a.x", "app.b.y"),
            edge("app.b.z", "app.a.y"),
            edge("app.a.x", "app.a.y"),
            edge("app.b.z", "app.c"),
            edge("elsewhere", "app.a"),
        ];
        let found = evaluate(&[rule], &edges, 20);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].nodes, ["app.a", "app.b"]);
        assert_eq!(found[0].count, 2);
        assert_eq!(found[0].architecture_edges.len(), 2);
        assert!(found[0]
            .architecture_edges
            .iter()
            .all(|e| !e.evidence.is_empty()));
    }
    #[test]
    fn scc_handles_disjoint_cycles_and_dag() {
        let graph = BTreeMap::from([
            ("a".into(), vec!["b".into()]),
            ("b".into(), vec!["a".into(), "c".into()]),
            ("c".into(), vec!["d".into()]),
            ("d".into(), vec!["c".into()]),
            ("e".into(), vec!["f".into()]),
            ("f".into(), vec![]),
        ]);
        assert_eq!(
            strongly_connected_components(&graph),
            vec![vec!["a", "b"], vec!["c", "d"], vec!["e"], vec!["f"]]
        );
    }
    #[test]
    fn self_edges_and_other_edge_kinds_are_ignored() {
        let rule = RuleConfig::NoCycles {
            id: "cycles".into(),
            within: "app".into(),
            edge_types: vec!["CALLS".into()],
        };
        assert!(evaluate(
            &[rule],
            &[edge("app.a", "app.b"), edge("app.b", "app.a")],
            20
        )
        .is_empty());
    }
}
