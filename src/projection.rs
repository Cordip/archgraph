//! The single semantic zoom implementation shared by show, context and HTTP.
use crate::{
    config::{immediate_child, is_within, parent_id, Interface, NodeKind},
    model::*,
    rules,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Architecture,
    File,
    DirectFiles,
    Boundary,
    /// An imported package (`provider.packages`), at its owner's level.
    Package,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSummary {
    pub id: String,
    pub title: String,
    pub kind: NodeKind,
    pub description: Option<String>,
    pub descendant_file_count: usize,
    pub observed_file_count: usize,
    pub interfaces: Vec<Interface>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub descendant_package_count: usize,
    pub entry_point_count: usize,
    pub no_observed_users_count: usize,
    /// No observed user outside the subtree and no entry point in it.
    pub no_outside_users: bool,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// Human-facing identity of a projection entry: the architecture ID or file
/// path, never the namespaced `node:`/`file:` UI identity.
pub fn display_name(node: &ProjectionNode) -> String {
    let id = node.architecture_id.as_deref().unwrap_or(&node.id);
    match node.entry_kind {
        EntryKind::File => node.file_path.clone().unwrap_or_else(|| node.id.clone()),
        EntryKind::Architecture => id.to_owned(),
        EntryKind::DirectFiles => format!("{id} (direct files)"),
        EntryKind::Boundary => format!("{id} (boundary)"),
        EntryKind::Package => node.id.clone(),
    }
}

/// What names an entry in a listing: its file path, package ID or
/// architecture ID.
pub fn identity(node: &ProjectionNode) -> &str {
    match node.entry_kind {
        EntryKind::Package => &node.id,
        _ => node
            .file_path
            .as_deref()
            .or(node.architecture_id.as_deref())
            .unwrap_or(&node.id),
    }
}

impl Projection {
    /// Display name for a projection node ID, e.g. an edge endpoint.
    pub fn endpoint_name(&self, id: &str) -> String {
        self.nodes
            .iter()
            .find(|node| node.id == id)
            .map(display_name)
            .unwrap_or_else(|| id.to_owned())
    }
}

/// "3 file(s)", with the packages a node owns, or what a package entry is.
pub fn size(node: &ProjectionNode) -> String {
    if let Some(package) = &node.package {
        return format!("{} package", package.ecosystem.title());
    }
    match node.package_count {
        0 => format!("{} file(s)", node.file_count),
        packages if node.file_count == 0 => format!("{packages} package(s)"),
        packages => format!("{} file(s), {packages} package(s)", node.file_count),
    }
}

/// " (N observed)" for architecture entries; empty for files and synthetic entries.
pub fn observed_suffix(node: &ProjectionNode) -> String {
    node.observed_file_count
        .map(|count| format!(" ({count} observed)"))
        .unwrap_or_default()
}

impl From<&CompiledNode> for NodeSummary {
    fn from(node: &CompiledNode) -> Self {
        Self {
            id: node.id.clone(),
            title: node.title.clone(),
            kind: node.kind,
            description: node.description.clone(),
            descendant_file_count: node.descendant_file_count,
            observed_file_count: node.observed_file_count,
            interfaces: node.interfaces.clone(),
            descendant_package_count: node.descendant_package_count,
            entry_point_count: node.entry_point_count,
            no_observed_users_count: node.no_observed_users_count,
            no_outside_users: node.no_outside_users(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionNode {
    /// Namespaced UI identity, never confused with an authored architecture ID.
    pub id: String,
    pub title: String,
    pub entry_kind: EntryKind,
    pub architecture_id: Option<String>,
    pub node_kind: Option<NodeKind>,
    pub file_path: Option<String>,
    pub file_count: usize,
    /// Architecture entries only: files with any observed dependency.
    pub observed_file_count: Option<usize>,
    pub description: Option<String>,
    pub interfaces: Vec<Interface>,
    pub outside_focus: bool,
    pub violation_rule_ids: Vec<String>,
    /// Architecture entries: imported packages their subtree owns.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub package_count: usize,
    /// Package entries: the package and every import of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageUse>,
    /// File entries: whether observed code uses the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<FileUsage>,
    /// Architecture and direct-file entries: declared entry points and files
    /// with no observed users among their files.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub entry_point_count: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub no_observed_users_count: usize,
    /// Architecture entries: distinct files outside the node's subtree that
    /// use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outside_user_count: Option<usize>,
    /// Architecture entries: `CompiledNode::no_outside_users`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_outside_users: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionEdge {
    #[serde(flatten)]
    pub edge: CompiledEdge,
    pub violation_rule_ids: Vec<String>,
    /// `no_cycles` rules whose cheapest cut includes this dependency.
    #[serde(default)]
    pub suggested_cut_rule_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Projection {
    pub focus: CompiledNode,
    pub breadcrumbs: Vec<NodeSummary>,
    pub nodes: Vec<ProjectionNode>,
    pub edges: Vec<ProjectionEdge>,
    pub violations: Vec<Violation>,
    pub evidence_limit: usize,
    pub evidence_notice: String,
    /// Inside entries (projection node IDs) grouped into rows from upper to
    /// lower layer, so observed dependencies mostly point downwards. Entries
    /// without dependencies form the last row. Empty for large leaf levels,
    /// where the UI falls back to a grid.
    #[serde(default)]
    pub layers: Vec<Vec<String>>,
}

/// Beyond this many inside entries (typically files of a large leaf) layering
/// is not worth its cost or readable, so none is computed.
const LAYERED_ENTRY_LIMIT: usize = 60;

/// Upper-to-lower rows: minimum-upward order, then longest-path ranks over
/// the dependencies that point downwards in that order.
fn layer_rows(nodes: &[ProjectionNode], edges: &[ProjectionEdge]) -> Vec<Vec<String>> {
    let inside: Vec<String> = nodes
        .iter()
        .filter(|node| !node.outside_focus)
        .map(|node| node.id.clone())
        .collect();
    if inside.is_empty() || inside.len() > LAYERED_ENTRY_LIMIT {
        return Vec::new();
    }
    let mut weights: BTreeMap<(String, String), usize> = BTreeMap::new();
    for projected in edges {
        let edge = &projected.edge;
        if edge.origin == EdgeOrigin::Observed
            && inside.contains(&edge.from)
            && inside.contains(&edge.to)
        {
            *weights
                .entry((edge.from.clone(), edge.to.clone()))
                .or_default() += edge.count;
        }
    }
    let connected: BTreeSet<&String> = weights.keys().flat_map(|(a, b)| [a, b]).collect();
    let order = rules::cheapest_layer_order(
        &connected.iter().map(|id| (*id).clone()).collect::<Vec<_>>(),
        &weights,
    );
    let position: BTreeMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let mut rank: BTreeMap<&str, usize> = BTreeMap::new();
    for id in &order {
        let row = weights
            .keys()
            .filter(|(from, to)| to == id && position[from.as_str()] < position[id.as_str()])
            .map(|(from, _)| rank[from.as_str()] + 1)
            .max()
            .unwrap_or(0);
        rank.insert(id.as_str(), row);
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    for id in &order {
        let row = rank[id.as_str()];
        if rows.len() <= row {
            rows.resize(row + 1, Vec::new());
        }
        rows[row].push(id.clone());
    }
    let isolated: Vec<String> = inside
        .into_iter()
        .filter(|id| !connected.contains(id))
        .collect();
    if !isolated.is_empty() {
        rows.push(isolated);
    }
    for row in &mut rows {
        row.sort();
    }
    rows
}

fn architecture_entry(node: &CompiledNode, outside: bool) -> ProjectionNode {
    ProjectionNode {
        id: format!("node:{}", node.id),
        title: node.title.clone(),
        entry_kind: EntryKind::Architecture,
        architecture_id: Some(node.id.clone()),
        node_kind: Some(node.kind),
        file_path: None,
        file_count: node.descendant_file_count,
        observed_file_count: Some(node.observed_file_count),
        description: node.description.clone(),
        interfaces: node.interfaces.clone(),
        outside_focus: outside,
        violation_rule_ids: Vec::new(),
        package_count: node.descendant_package_count,
        package: None,
        usage: None,
        entry_point_count: node.entry_point_count,
        no_observed_users_count: node.no_observed_users_count,
        outside_user_count: Some(node.outside_user_count),
        no_outside_users: node.no_outside_users(),
    }
}
fn file_entry(path: &str, node: &CompiledNode, usage: Option<FileUsage>) -> ProjectionNode {
    ProjectionNode {
        id: format!("file:{path}"),
        title: path.into(),
        entry_kind: EntryKind::File,
        architecture_id: Some(node.id.clone()),
        node_kind: None,
        file_path: Some(path.into()),
        file_count: 1,
        observed_file_count: None,
        description: None,
        interfaces: Vec::new(),
        outside_focus: false,
        violation_rule_ids: Vec::new(),
        package_count: 0,
        package: None,
        usage,
        entry_point_count: 0,
        no_observed_users_count: 0,
        outside_user_count: None,
        no_outside_users: false,
    }
}
/// An imported package, drawn at the level of the node owning it.
fn package_entry(id: &str, owner: &CompiledNode, package: Option<&PackageUse>) -> ProjectionNode {
    ProjectionNode {
        id: id.into(),
        title: package.map_or_else(
            || id.trim_start_matches(PACKAGE_PREFIX).to_owned(),
            |package| package.name.clone(),
        ),
        entry_kind: EntryKind::Package,
        architecture_id: Some(owner.id.clone()),
        node_kind: None,
        file_path: None,
        file_count: 0,
        observed_file_count: None,
        description: package.map(|package| {
            format!(
                "{} package imported by {} file(s).",
                package.ecosystem.title(),
                package
                    .imports
                    .iter()
                    .map(|import| &import.file)
                    .collect::<BTreeSet<_>>()
                    .len()
            )
        }),
        interfaces: Vec::new(),
        outside_focus: false,
        violation_rule_ids: Vec::new(),
        package_count: 0,
        package: package.cloned(),
        usage: None,
        entry_point_count: 0,
        no_observed_users_count: 0,
        outside_user_count: None,
        no_outside_users: false,
    }
}
fn direct_entry(ir: &ArchitectureIr, focus: &CompiledNode) -> ProjectionNode {
    let count = |wanted: FileUsage| {
        focus
            .direct_files
            .iter()
            .filter(|path| ir.file_usage(path) == Some(wanted))
            .count()
    };
    ProjectionNode {
        id: format!("direct:{}", focus.id),
        title: "Directly owned files".into(),
        entry_kind: EntryKind::DirectFiles,
        architecture_id: Some(focus.id.clone()),
        node_kind: Some(focus.kind),
        file_path: None,
        file_count: focus.direct_files.len(),
        observed_file_count: None,
        description: Some(
            "Files mapped directly to this focus, rather than an authored child.".into(),
        ),
        interfaces: Vec::new(),
        outside_focus: false,
        violation_rule_ids: Vec::new(),
        package_count: 0,
        package: None,
        usage: None,
        entry_point_count: count(FileUsage::EntryPoint),
        no_observed_users_count: count(FileUsage::NoObservedUsers),
        outside_user_count: None,
        no_outside_users: false,
    }
}
fn boundary_entry(focus: &CompiledNode) -> ProjectionNode {
    ProjectionNode { id: format!("boundary:{}", focus.id), title: format!("{} (boundary)", focus.title), entry_kind: EntryKind::Boundary,
        architecture_id: Some(focus.id.clone()), node_kind: Some(focus.kind), file_path: None,
        file_count: focus.descendant_file_count,
        observed_file_count: None,
        description: Some("An authored manual edge names the focus itself; it cannot be attributed to a particular child or file.".into()),
        interfaces: focus.interfaces.clone(), outside_focus: false, violation_rule_ids: Vec::new(),
        package_count: 0, package: None, usage: None, entry_point_count: 0,
        no_observed_users_count: 0, outside_user_count: None, no_outside_users: false }
}

type Packages<'a> = BTreeMap<&'a str, &'a PackageUse>;

fn representative(
    ir: &ArchitectureIr,
    packages: &Packages,
    focus: &CompiledNode,
    owner: &str,
    file: Option<&str>,
) -> Result<ProjectionNode> {
    if !is_within(owner, &focus.id) {
        let outside = ir
            .nodes
            .get(owner)
            .with_context(|| format!("IR edge references missing node `{owner}`"))?;
        // Retain the actual outside owner, including external services. Do not
        // invent containment for unrelated architecture roots.
        return Ok(architecture_entry(outside, true));
    }
    if let Some(child) = immediate_child(owner, &focus.id) {
        let child = ir
            .nodes
            .get(child)
            .context("IR immediate child is missing")?;
        return Ok(architecture_entry(child, false));
    }
    match file {
        // Packages are few and not files: each is its own entry, also next
        // to authored children.
        Some(id) if id.starts_with(PACKAGE_PREFIX) && ir.packages.is_some() => {
            Ok(package_entry(id, focus, packages.get(id).copied()))
        }
        Some(path) if focus.children.is_empty() => Ok(file_entry(path, focus, ir.file_usage(path))),
        Some(_) => Ok(direct_entry(ir, focus)),
        None => Ok(boundary_entry(focus)),
    }
}

type EdgeKey = (String, String, String);
#[derive(Default)]
struct ViolationIndex {
    exact: BTreeMap<EdgeKey, BTreeSet<String>>,
    subtrees: BTreeMap<EdgeKey, BTreeSet<String>>,
    cuts: BTreeMap<EdgeKey, BTreeSet<String>>,
}

/// Rules an observation violates, and rules whose suggested cut contains it.
#[derive(Clone, Default)]
struct EdgeMarks {
    violations: BTreeSet<String>,
    cuts: BTreeSet<String>,
}
impl ViolationIndex {
    fn new(violations: &[Violation]) -> Self {
        let mut result = Self::default();
        for violation in violations {
            let table = if violation.kind == "no_cycles" {
                &mut result.subtrees
            } else {
                &mut result.exact
            };
            for edge in &violation.architecture_edges {
                let key = (edge.from.clone(), edge.to.clone(), edge.kind.clone());
                let cut = violation
                    .suggested_cuts
                    .iter()
                    .any(|cut| cut.from == edge.from && cut.to == edge.to);
                if cut {
                    result
                        .cuts
                        .entry(key.clone())
                        .or_default()
                        .insert(violation.rule_id.clone());
                }
                table
                    .entry(key)
                    .or_default()
                    .insert(violation.rule_id.clone());
            }
        }
        result
    }
    fn matching(&self, edge: &ResolvedEdge) -> EdgeMarks {
        let key = (edge.from.clone(), edge.to.clone(), edge.kind.clone());
        let mut found = EdgeMarks {
            violations: self.exact.get(&key).cloned().unwrap_or_default(),
            cuts: BTreeSet::new(),
        };
        let mut from = Some(edge.from.as_str());
        while let Some(a) = from {
            let mut to = Some(edge.to.as_str());
            while let Some(b) = to {
                let key = (a.to_owned(), b.to_owned(), edge.kind.clone());
                if let Some(ids) = self.subtrees.get(&key) {
                    found.violations.extend(ids.iter().cloned());
                }
                if let Some(ids) = self.cuts.get(&key) {
                    found.cuts.extend(ids.iter().cloned());
                }
                to = parent_id(b);
            }
            from = parent_id(a);
        }
        found
    }
}

#[derive(Default)]
struct ProjectedAccumulator {
    observed: EvidenceAccumulator,
    manual: Vec<crate::config::ManualEdgeConfig>,
    violations: BTreeSet<String>,
    cuts: BTreeSet<String>,
}

pub fn project(ir: &ArchitectureIr, focus_id: &str, evidence_limit: usize) -> Result<Projection> {
    let focus = ir.nodes.get(focus_id)
        .with_context(|| format!("unknown architecture node `{focus_id}`; use `archgraph show` or UI search to find a valid ID"))?;
    // Recompute only when a caller requests a different evidence sample cap.
    let all_violations = if evidence_limit == EVIDENCE_LIMIT {
        ir.violations.clone()
    } else {
        rules::evaluate(&ir.rules, &ir.resolved_edges, evidence_limit)
    };
    let index = ViolationIndex::new(&all_violations);
    let mut rule_cache: BTreeMap<EdgeKey, EdgeMarks> = BTreeMap::new();
    let packages: Packages = ir
        .packages
        .iter()
        .flat_map(|report| &report.packages)
        .map(|package| (package.id.as_str(), package))
        .collect();
    let mut entries: BTreeMap<String, ProjectionNode> = BTreeMap::new();
    for id in &focus.packages {
        let entry = package_entry(id, focus, packages.get(id.as_str()).copied());
        entries.insert(entry.id.clone(), entry);
    }
    if focus.children.is_empty() {
        for path in &focus.direct_files {
            let entry = file_entry(path, focus, ir.file_usage(path));
            entries.insert(entry.id.clone(), entry);
        }
    } else {
        for id in &focus.children {
            let child = ir.nodes.get(id).context("IR focus child is missing")?;
            let entry = architecture_entry(child, false);
            entries.insert(entry.id.clone(), entry);
        }
        // Mapping to a non-leaf is valid. Never make its directly owned files
        // disappear just because this focus also has authored children.
        if !focus.direct_files.is_empty() {
            let entry = direct_entry(ir, focus);
            entries.insert(entry.id.clone(), entry);
        }
    }
    let mut groups: BTreeMap<(String, String, String, EdgeOrigin), ProjectedAccumulator> =
        BTreeMap::new();
    for edge in &ir.resolved_edges {
        if !is_within(&edge.from, focus_id) && !is_within(&edge.to, focus_id) {
            continue;
        }
        let from = representative(
            ir,
            &packages,
            focus,
            &edge.from,
            Some(&edge.evidence.from_file),
        )?;
        let to = representative(ir, &packages, focus, &edge.to, Some(&edge.evidence.to_file))?;
        let from_id = from.id.clone();
        let to_id = to.id.clone();
        let marks = rule_cache
            .entry((edge.from.clone(), edge.to.clone(), edge.kind.clone()))
            .or_insert_with(|| index.matching(edge));
        for entry in [from, to] {
            let entry = entries.entry(entry.id.clone()).or_insert(entry);
            entry
                .violation_rule_ids
                .extend(marks.violations.iter().cloned());
        }
        if from_id == to_id {
            continue;
        }
        let accumulator = groups
            .entry((from_id, to_id, edge.kind.clone(), EdgeOrigin::Observed))
            .or_default();
        accumulator.observed.add(&edge.evidence, evidence_limit);
        accumulator
            .violations
            .extend(marks.violations.iter().cloned());
        accumulator.cuts.extend(marks.cuts.iter().cloned());
    }
    for edge in ir
        .edges
        .iter()
        .filter(|edge| edge.origin == EdgeOrigin::Manual)
    {
        if !is_within(&edge.from, focus_id) && !is_within(&edge.to, focus_id) {
            continue;
        }
        let from = representative(ir, &packages, focus, &edge.from, None)?;
        let to = representative(ir, &packages, focus, &edge.to, None)?;
        let from_id = from.id.clone();
        let to_id = to.id.clone();
        entries.entry(from_id.clone()).or_insert(from);
        entries.entry(to_id.clone()).or_insert(to);
        if from_id == to_id {
            continue;
        }
        groups
            .entry((from_id, to_id, edge.kind.clone(), EdgeOrigin::Manual))
            .or_default()
            .manual
            .extend(edge.manual_edges.iter().cloned());
    }
    let violations: Vec<Violation> = all_violations
        .into_iter()
        .filter(|violation| {
            rules::violation_touches(violation, focus_id)
                || entries.values().any(|entry| {
                    entry
                        .architecture_id
                        .as_deref()
                        .is_some_and(|id| rules::violation_touches(violation, id))
                })
        })
        .collect();
    for entry in entries.values_mut() {
        if entry.entry_kind == EntryKind::Architecture || entry.entry_kind == EntryKind::Boundary {
            if let Some(id) = &entry.architecture_id {
                for violation in &violations {
                    if rules::violation_touches(violation, id) {
                        entry.violation_rule_ids.push(violation.rule_id.clone());
                    }
                }
            }
        }
        entry.violation_rule_ids.sort();
        entry.violation_rule_ids.dedup();
    }
    let edges: Vec<ProjectionEdge> = groups
        .into_iter()
        .map(|((from, to, kind, origin), mut aggregate)| {
            aggregate.manual.sort_by(|a, b| a.id.cmp(&b.id));
            let mut edge = aggregate.observed.into_edge(from, to, kind);
            edge.origin = origin;
            if origin == EdgeOrigin::Manual {
                edge.count = aggregate.manual.len();
            }
            edge.manual_edges = aggregate.manual;
            ProjectionEdge {
                edge,
                violation_rule_ids: aggregate.violations.into_iter().collect(),
                suggested_cut_rule_ids: aggregate.cuts.into_iter().collect(),
            }
        })
        .collect();
    let mut breadcrumbs = Vec::new();
    let mut current = Some(focus_id);
    while let Some(id) = current {
        breadcrumbs.push(NodeSummary::from(
            ir.nodes
                .get(id)
                .context("IR breadcrumb ancestor is missing")?,
        ));
        current = parent_id(id);
    }
    breadcrumbs.reverse();
    // Inside entries first; the remainder are visibly marked cross-boundary.
    let mut nodes: Vec<_> = entries.into_values().collect();
    nodes.sort_by(|a, b| (a.outside_focus, &a.id).cmp(&(b.outside_focus, &b.id)));
    let layers = layer_rows(&nodes, &edges);
    Ok(Projection {
        focus: focus.clone(),
        breadcrumbs,
        nodes,
        edges,
        violations,
        evidence_limit,
        evidence_notice: ir.evidence_notice.clone(),
        layers,
    })
}
