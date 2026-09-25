//! Rules operate only on observed edges. Manual architectural intent is never
//! silently treated as source-code evidence.
use crate::{
    config::{immediate_child, is_within, Layer, RuleConfig},
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
            RuleConfig::Layers {
                layers, edge_types, ..
            } => {
                let upward = edges.iter().filter(|edge| {
                    edge_types.contains(&edge.kind)
                        && matches!(
                            (layer_index(layers, &edge.from), layer_index(layers, &edge.to)),
                            (Some(from), Some(to)) if to < from
                        )
                });
                violations.extend(dependency_violations(rule, upward, limit));
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
                    let mut weights: BTreeMap<(String, String), usize> = BTreeMap::new();
                    for edge in &participating {
                        *weights
                            .entry((edge.from.clone(), edge.to.clone()))
                            .or_default() += 1;
                    }
                    let layer_order = cheapest_layer_order(&components[index], &weights);
                    let suggested_cuts = upward_dependencies(&layer_order, &weights);
                    let cut_count: usize = suggested_cuts.iter().map(|cut| cut.count).sum();
                    let cut_text = suggested_cuts
                        .iter()
                        .map(|cut| format!("{} -> {} x{}", cut.from, cut.to, cut.count))
                        .collect::<Vec<_>>()
                        .join(", ");
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
                        message: format!("cycle among immediate children of `{within}`: {}; {} observed dependencies participate (SCC, not an ordered cycle path). Cheapest cut ({cut_count} of {}): {cut_text}; resulting layers, upper to lower: {}", components[index].join(", "), evidence.count, evidence.count, layer_order.join(" > ")),
                        evidence: evidence.evidence, architecture_edges, layer_order, suggested_cuts,
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

/// The layer (0 = upper) whose member is `node` or one of its ancestors.
fn layer_index(layers: &[Layer], node: &str) -> Option<usize> {
    layers
        .iter()
        .position(|layer| layer.nodes().iter().any(|member| is_within(node, member)))
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
                layer_order: Vec::new(),
                suggested_cuts: Vec::new(),
            }
        })
        .collect()
}

/// Every resolved observation behind `violation`, not just its bounded
/// evidence sample. `rule` must be the rule that produced the violation.
pub fn violation_observations<'a>(
    rule: &RuleConfig,
    violation: &Violation,
    edges: &'a [ResolvedEdge],
) -> Vec<&'a ResolvedEdge> {
    match rule {
        // Dependency rules decide by owner nodes and kind alone, and group
        // violations by exactly those, so equal owners and kind means included.
        RuleConfig::DenyDependency { .. }
        | RuleConfig::AllowOnly { .. }
        | RuleConfig::Layers { .. } => edges
            .iter()
            .filter(|edge| {
                violation.from.as_ref() == Some(&edge.from)
                    && violation.to.as_ref() == Some(&edge.to)
                    && violation.edge_kind.as_ref() == Some(&edge.kind)
            })
            .collect(),
        RuleConfig::NoCycles {
            within, edge_types, ..
        } => edges
            .iter()
            .filter(|edge| {
                let from = immediate_child(&edge.from, within);
                let to = immediate_child(&edge.to, within);
                edge_types.contains(&edge.kind)
                    && from != to
                    && [from, to].iter().all(|member| {
                        member.is_some_and(|id| violation.nodes.iter().any(|node| node == id))
                    })
            })
            .collect(),
    }
}

/// Orders SCC members from upper to lower layer, minimizing the observations
/// that point upwards (a minimum-weight feedback arc set). Exact by dynamic
/// programming over subsets up to `EXACT_ORDER_LIMIT` members, else the greedy
/// Eades-Lin-Smyth heuristic. Ties resolve by node ID, so output is stable.
pub fn cheapest_layer_order(
    nodes: &[String],
    weights: &BTreeMap<(String, String), usize>,
) -> Vec<String> {
    const EXACT_ORDER_LIMIT: usize = 16;
    let mut nodes = nodes.to_vec();
    nodes.sort();
    let n = nodes.len();
    let matrix: Vec<Vec<usize>> = (0..n)
        .map(|a| {
            (0..n)
                .map(|b| {
                    weights
                        .get(&(nodes[a].clone(), nodes[b].clone()))
                        .copied()
                        .unwrap_or(0)
                })
                .collect()
        })
        .collect();
    let order = if n <= EXACT_ORDER_LIMIT {
        exact_order(&matrix)
    } else {
        greedy_order(&matrix)
    };
    order
        .into_iter()
        .map(|index| nodes[index].clone())
        .collect()
}

fn exact_order(matrix: &[Vec<usize>]) -> Vec<usize> {
    let n = matrix.len();
    // best[mask]: cheapest cost of placing exactly `mask` as the upper layers.
    // Appending v below them costs every dependency from v up into `mask`.
    let full = (1usize << n) - 1;
    let mut best = vec![usize::MAX; full + 1];
    let mut last = vec![0usize; full + 1];
    best[0] = 0;
    for mask in 0..full {
        if best[mask] == usize::MAX {
            continue;
        }
        for v in (0..n).filter(|v| mask & (1 << v) == 0) {
            let upward: usize = (0..n)
                .filter(|u| mask & (1 << u) != 0)
                .map(|u| matrix[v][u])
                .sum();
            let next = mask | (1 << v);
            if best[mask] + upward < best[next] {
                best[next] = best[mask] + upward;
                last[next] = v;
            }
        }
    }
    let mut order = Vec::with_capacity(n);
    let mut mask = full;
    while mask != 0 {
        order.push(last[mask]);
        mask &= !(1 << last[mask]);
    }
    order.reverse();
    order
}

fn greedy_order(matrix: &[Vec<usize>]) -> Vec<usize> {
    let mut remaining: BTreeSet<usize> = (0..matrix.len()).collect();
    let (mut upper, mut lower) = (Vec::new(), Vec::new());
    let out = |v: usize, rest: &BTreeSet<usize>| rest.iter().map(|&u| matrix[v][u]).sum::<usize>();
    let inn = |v: usize, rest: &BTreeSet<usize>| rest.iter().map(|&u| matrix[u][v]).sum::<usize>();
    while !remaining.is_empty() {
        let next = if let Some(&sink) = remaining.iter().find(|&&v| out(v, &remaining) == 0) {
            lower.push(sink);
            sink
        } else if let Some(&source) = remaining.iter().find(|&&v| inn(v, &remaining) == 0) {
            upper.push(source);
            source
        } else {
            let pick = *remaining
                .iter()
                .max_by_key(|&&v| {
                    let delta = out(v, &remaining) as i64 - inn(v, &remaining) as i64;
                    (delta, std::cmp::Reverse(v))
                })
                .expect("remaining is nonempty");
            upper.push(pick);
            pick
        };
        remaining.remove(&next);
    }
    upper.extend(lower.into_iter().rev());
    upper
}

/// Dependencies pointing from a lower to an upper layer of `order`.
pub fn upward_dependencies(
    order: &[String],
    weights: &BTreeMap<(String, String), usize>,
) -> Vec<CycleCut> {
    let position: BTreeMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let mut cuts: Vec<CycleCut> = weights
        .iter()
        .filter(|((from, to), &count)| {
            count > 0 && position.get(from.as_str()) > position.get(to.as_str())
        })
        .map(|((from, to), &count)| CycleCut {
            from: from.clone(),
            to: to.clone(),
            count,
        })
        .collect();
    cuts.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| (&a.from, &a.to).cmp(&(&b.from, &b.to)))
    });
    cuts
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
    fn layers_reject_only_upward_dependencies_with_complete_observations() {
        let rule = RuleConfig::Layers {
            id: "layers".into(),
            layers: vec![
                Layer::Many(vec!["app.web".into(), "app.cli".into()]),
                Layer::One("app.domain".into()),
                Layer::One("app.db".into()),
            ],
            edge_types: vec!["IMPORTS".into()],
        };
        let edges = vec![
            edge("app.web.x", "app.domain"),   // down: fine
            edge("app.web.x", "app.db.y"),     // skipping a layer: fine
            edge("app.web.x", "app.cli"),      // peers: fine
            edge("app.domain.a", "app.web.x"), // up
            edge("app.db.y", "app.domain.b"),  // up
            edge("app.db.z", "app.domain.b"),  // up, same owners as above
            edge("app.db.y", "app.other"),     // unlisted target: fine
            edge("app.other", "app.web"),      // unlisted source: fine
        ];
        let found = evaluate(std::slice::from_ref(&rule), &edges, 20);
        let pairs: Vec<_> = found
            .iter()
            .map(|v| (v.from.clone().unwrap(), v.to.clone().unwrap(), v.count))
            .collect();
        assert_eq!(
            pairs,
            [
                ("app.db.y".into(), "app.domain.b".into(), 1),
                ("app.db.z".into(), "app.domain.b".into(), 1),
                ("app.domain.a".into(), "app.web.x".into(), 1),
            ]
        );
        assert!(found.iter().all(|v| v.kind == "layers"));
        for violation in &found {
            assert_eq!(violation_observations(&rule, violation, &edges).len(), 1);
        }
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
    fn weights(pairs: &[(&str, &str, usize)]) -> BTreeMap<(String, String), usize> {
        pairs
            .iter()
            .map(|(a, b, w)| ((a.to_string(), b.to_string()), *w))
            .collect()
    }
    fn upward_cost(order: &[String], weights: &BTreeMap<(String, String), usize>) -> usize {
        upward_dependencies(order, weights)
            .iter()
            .map(|cut| cut.count)
            .sum()
    }
    fn permutations(items: Vec<String>) -> Vec<Vec<String>> {
        if items.len() <= 1 {
            return vec![items];
        }
        let mut result = Vec::new();
        for i in 0..items.len() {
            let mut rest = items.clone();
            let head = rest.remove(i);
            for mut tail in permutations(rest) {
                tail.insert(0, head.clone());
                result.push(tail);
            }
        }
        result
    }
    #[test]
    fn cheapest_cut_keeps_the_heavy_direction() {
        // Shaped like zammad's backend: models use lib heavily, lib uses models less.
        let w = weights(&[
            ("jobs", "lib", 13),
            ("jobs", "models", 1),
            ("lib", "models", 42),
            ("models", "lib", 114),
            ("models", "jobs", 1),
            ("services", "lib", 24),
            ("lib", "services", 1),
        ]);
        let nodes: Vec<String> = ["jobs", "lib", "models", "services"]
            .map(String::from)
            .to_vec();
        let order = cheapest_layer_order(&nodes, &w);
        let cuts = upward_dependencies(&order, &w);
        assert_eq!(
            cuts[0],
            CycleCut {
                from: "lib".into(),
                to: "models".into(),
                count: 42
            }
        );
        assert_eq!(upward_cost(&order, &w), 44);
    }
    #[test]
    fn exact_order_is_optimal_and_greedy_order_is_acyclic() {
        // Deterministic pseudo-random dense graphs, checked against brute force.
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        for size in 2..=6 {
            let nodes: Vec<String> = (0..size).map(|i| format!("n{i}")).collect();
            let mut pairs = Vec::new();
            for a in &nodes {
                for b in &nodes {
                    if a != b {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        if !seed.is_multiple_of(3) {
                            pairs.push((a.clone(), b.clone(), (seed % 50) as usize + 1));
                        }
                    }
                }
            }
            let w: BTreeMap<_, _> = pairs.into_iter().map(|(a, b, c)| ((a, b), c)).collect();
            let optimum = permutations(nodes.clone())
                .iter()
                .map(|order| upward_cost(order, &w))
                .min()
                .unwrap();
            assert_eq!(
                upward_cost(&cheapest_layer_order(&nodes, &w), &w),
                optimum,
                "size {size}"
            );
            let order: Vec<String> = greedy_order(
                &(0..size)
                    .map(|a| {
                        (0..size)
                            .map(|b| {
                                w.get(&(nodes[a].clone(), nodes[b].clone()))
                                    .copied()
                                    .unwrap_or(0)
                            })
                            .collect()
                    })
                    .collect::<Vec<Vec<usize>>>(),
            )
            .into_iter()
            .map(|i| nodes[i].clone())
            .collect();
            let mut rest = w.clone();
            for cut in upward_dependencies(&order, &w) {
                rest.remove(&(cut.from, cut.to));
            }
            let mut graph: BTreeMap<String, Vec<String>> =
                nodes.iter().map(|n| (n.clone(), Vec::new())).collect();
            for (a, b) in rest.keys() {
                graph.get_mut(a).unwrap().push(b.clone());
            }
            assert!(strongly_connected_components(&graph)
                .iter()
                .all(|c| c.len() == 1));
        }
    }
    #[test]
    fn cycle_violation_names_the_cheapest_cut() {
        let rule = RuleConfig::NoCycles {
            id: "cycles".into(),
            within: "app".into(),
            edge_types: vec!["IMPORTS".into()],
        };
        let edges = vec![
            edge("app.a.x", "app.b.y"),
            edge("app.a.z", "app.b.y"),
            edge("app.b.z", "app.a.y"),
        ];
        let found = evaluate(&[rule], &edges, 20);
        assert_eq!(found[0].layer_order, ["app.a", "app.b"]);
        assert_eq!(
            found[0].suggested_cuts,
            [CycleCut {
                from: "app.b".into(),
                to: "app.a".into(),
                count: 1
            }]
        );
        assert!(
            found[0]
                .message
                .contains("Cheapest cut (1 of 3): app.b -> app.a x1"),
            "{}",
            found[0].message
        );
    }
    #[test]
    fn violation_observations_are_complete_beyond_the_evidence_cap() {
        let rules = [
            RuleConfig::DenyDependency {
                id: "deny".into(),
                from: "app.a".into(),
                to: "app.b".into(),
                edge_types: vec!["IMPORTS".into()],
                include_descendants: true,
            },
            RuleConfig::NoCycles {
                id: "cycles".into(),
                within: "app".into(),
                edge_types: vec!["IMPORTS".into()],
            },
        ];
        let mut edges: Vec<_> = (0..30)
            .map(|i| {
                let mut e = edge("app.a", "app.b");
                e.evidence.from_file = format!("a{i}.rs");
                e
            })
            .collect();
        edges.push(edge("app.b", "app.a"));
        edges.push(edge("app.c", "app.a"));
        let found = evaluate(&rules, &edges, 5);
        let deny = found.iter().find(|v| v.rule_id == "deny").unwrap();
        assert_eq!(deny.evidence.len(), 5);
        assert_eq!(violation_observations(&rules[0], deny, &edges).len(), 30);
        let cycle = found.iter().find(|v| v.rule_id == "cycles").unwrap();
        assert_eq!(violation_observations(&rules[1], cycle, &edges).len(), 31);
    }
}
