use crate::config::{Interface, ManualEdgeConfig, NodeKind, ProjectConfig, RuleConfig};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, collections::BTreeMap};

pub const EVIDENCE_LIMIT: usize = 20;
pub const EVIDENCE_NOTICE: &str = "Only observed dependencies are checked. No observed edge is not proof that no runtime dependency exists.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderInfo {
    pub provider: String,
    pub version: Option<String>,
    pub repository: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodeEdge {
    pub from_file: String,
    pub to_file: String,
    pub kind: String,
    pub confidence: Option<f64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EdgeEvidence {
    pub from_file: String,
    pub to_file: String,
    pub kind: String,
    pub confidence: Option<f64>,
    pub reason: Option<String>,
}
impl From<CodeEdge> for EdgeEvidence {
    fn from(edge: CodeEdge) -> Self {
        Self {
            from_file: edge.from_file,
            to_file: edge.to_file,
            kind: edge.kind,
            confidence: edge.confidence,
            reason: edge.reason,
        }
    }
}

pub fn evidence_cmp(a: &EdgeEvidence, b: &EdgeEvidence) -> Ordering {
    (&a.from_file, &a.to_file, &a.kind)
        .cmp(&(&b.from_file, &b.to_file, &b.kind))
        .then_with(|| match (a.confidence, b.confidence) {
            (Some(a), Some(b)) => a.total_cmp(&b),
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
        .then_with(|| a.reason.cmp(&b.reason))
}

/// Stores all resolved IMPORTS, not the entire provider graph. Needed for lossless
/// leaf-file projections and for context --evidence-limit beyond the IR sample cap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub evidence: EdgeEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledNode {
    pub id: String,
    pub title: String,
    pub kind: NodeKind,
    pub description: Option<String>,
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub direct_files: Vec<String>,
    pub descendant_file_count: usize,
    /// Descendant files with at least one resolved observed dependency, in
    /// either direction. A low ratio means a clean check is weak evidence.
    pub observed_file_count: usize,
    /// Descendant files absent from the provider's index altogether.
    #[serde(default)]
    pub unindexed_file_count: usize,
    pub interfaces: Vec<Interface>,
    /// Imported packages (`provider.packages`) this node owns directly, by
    /// package ID; they are not files and count in no file total.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub descendant_package_count: usize,
    /// Descendant files declared in `project.entry_points`.
    #[serde(default)]
    pub entry_point_count: usize,
    /// Descendant files with `FileUsage::NoObservedUsers`.
    #[serde(default)]
    pub no_observed_users_count: usize,
    /// Distinct files outside this subtree (mapped or not) with an observed
    /// dependency on a file inside it. Zero, with no entry point and indexed
    /// files, makes the whole subtree a dead-code candidate.
    #[serde(default)]
    pub outside_user_count: usize,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledFile {
    pub path: String,
    pub node: Option<String>,
    pub ambiguous_matches: Vec<String>,
    /// Mapped files only: whether observed code uses this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<FileUsage>,
}

pub const USAGE_NOTICE: &str = "No observed users is not proof of dead code: GitNexus misses Ruby autoloading, HTML script tags, dynamic imports, framework conventions and tools that load files by name (see docs/gitnexus-limitations.md). Declare such files in project.entry_points.";

/// Whether any observed code uses a mapped file. Observations are the
/// configured `provider.edge_types` after the reason and confidence
/// filters; a file's dependency on itself and imports of packages do not
/// count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileUsage {
    /// Declared in `project.entry_points`: a tool or runtime loads it.
    EntryPoint,
    /// Another file has an observed dependency on it, from inside the
    /// architecture or not (an excluded or unassigned file is a user too).
    Used,
    /// Not declared, and nothing observed depends on it: an entry point
    /// nobody declared, code loaded in a way the provider does not see, or
    /// dead code. A candidate, never a proof.
    NoObservedUsers,
    /// Not in the provider's index, so nothing about its users is known.
    NotIndexed,
}

impl ArchitectureIr {
    /// The usage of a file by path (`files` is sorted by path).
    pub fn file_usage(&self, path: &str) -> Option<FileUsage> {
        self.files
            .binary_search_by(|file| file.path.as_str().cmp(path))
            .ok()
            .and_then(|index| self.files[index].usage)
    }
}

impl FileUsage {
    pub fn describe(self) -> &'static str {
        match self {
            Self::EntryPoint => "entry point",
            Self::Used => "used",
            Self::NoObservedUsers => "no observed users",
            Self::NotIndexed => "not indexed",
        }
    }
}

impl CompiledNode {
    /// Nothing observed outside the subtree uses it, it declares no entry
    /// point, and the provider indexed some of its files: the whole node
    /// may be unused. Top-level nodes have no outside, so they never are.
    pub fn no_outside_users(&self) -> bool {
        self.parent.is_some()
            && self.descendant_file_count > self.unindexed_file_count
            && self.entry_point_count == 0
            && self.outside_user_count == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeOrigin {
    Observed,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub origin: EdgeOrigin,
    pub count: usize,
    pub evidence: Vec<EdgeEvidence>,
    pub confidence_min: Option<f64>,
    pub confidence_max: Option<f64>,
    pub manual_edges: Vec<ManualEdgeConfig>,
}

/// Bounded, deterministic sampling. Count includes duplicate observations;
/// confidence range is computed over all observations, not just sampled ones.
#[derive(Debug, Default)]
pub struct EvidenceAccumulator {
    pub count: usize,
    pub evidence: Vec<EdgeEvidence>,
    pub confidence_min: Option<f64>,
    pub confidence_max: Option<f64>,
}
impl EvidenceAccumulator {
    pub fn add(&mut self, evidence: &EdgeEvidence, limit: usize) {
        self.count += 1;
        if let Some(confidence) = evidence.confidence {
            self.confidence_min = Some(
                self.confidence_min
                    .map_or(confidence, |x| x.min(confidence)),
            );
            self.confidence_max = Some(
                self.confidence_max
                    .map_or(confidence, |x| x.max(confidence)),
            );
        }
        if limit == 0 {
            return;
        }
        let position = self
            .evidence
            .binary_search_by(|current| evidence_cmp(current, evidence))
            .unwrap_or_else(|position| position);
        if position < limit {
            self.evidence.insert(position, evidence.clone());
            self.evidence.truncate(limit);
        }
    }
    pub fn into_edge(self, from: String, to: String, kind: String) -> CompiledEdge {
        CompiledEdge {
            from,
            to,
            kind,
            origin: EdgeOrigin::Observed,
            count: self.count,
            evidence: self.evidence,
            confidence_min: self.confidence_min,
            confidence_max: self.confidence_max,
            manual_edges: Vec::new(),
        }
    }
}

pub fn aggregate_observed<'a>(
    edges: impl IntoIterator<Item = &'a ResolvedEdge>,
    limit: usize,
) -> Vec<CompiledEdge> {
    let mut groups: BTreeMap<(String, String, String), EvidenceAccumulator> = BTreeMap::new();
    for edge in edges {
        groups
            .entry((edge.from.clone(), edge.to.clone(), edge.kind.clone()))
            .or_default()
            .add(&edge.evidence, limit);
    }
    groups
        .into_iter()
        .map(|((from, to, kind), evidence)| evidence.into_edge(from, to, kind))
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Violation {
    pub rule_id: String,
    pub kind: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub edge_kind: Option<String>,
    pub nodes: Vec<String>,
    /// Actual ownership nodes touched by the violating observations; unlike SCC
    /// display nodes, these permit precise subtree filtering at deeper focuses.
    pub affected_nodes: Vec<String>,
    pub count: usize,
    pub message: String,
    pub evidence: Vec<EdgeEvidence>,
    /// Every participating architecture edge, each with its own bounded sample.
    pub architecture_edges: Vec<CompiledEdge>,
    /// `no_cycles` only: SCC members ordered from upper to lower layer so that
    /// the fewest observations point upwards.
    #[serde(default)]
    pub layer_order: Vec<String>,
    /// `no_cycles` only: the upward dependencies in `layer_order`. Removing them
    /// breaks the cycle at the lowest observed cost; most significant first.
    #[serde(default)]
    pub suggested_cuts: Vec<CycleCut>,
}

/// One architecture-level dependency (all participating relation kinds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleCut {
    pub from: String,
    pub to: String,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    /// Rule nodes below `policies.min_observed_ratio`.
    #[serde(default)]
    pub low_coverage: Vec<CoverageIssue>,
    pub unassigned_files: Vec<String>,
    pub ambiguous_files: BTreeMap<String, Vec<String>>,
    pub provider_anomalies: Vec<ProviderAnomaly>,
    /// Mapped files the provider did not index (empty if it cannot list files).
    #[serde(default)]
    pub unindexed_files: Vec<String>,
    pub warnings: Vec<String>,
}

/// A rule refers to a node whose files are mostly invisible to the provider.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CoverageIssue {
    pub rule_id: String,
    pub node: String,
    pub observed_files: usize,
    /// Files the provider never indexed; they cannot show dependencies.
    #[serde(default)]
    pub unindexed_files: usize,
    pub total_files: usize,
}

impl std::fmt::Display for CoverageIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rule `{}`: only {} of {} files under `{}` ({:.0}%) have any observed dependency{}; a passing check there is weak evidence",
            self.rule_id,
            self.observed_files,
            self.total_files,
            self.node,
            100.0 * self.observed_files as f64 / self.total_files as f64,
            if self.unindexed_files == 0 {
                String::new()
            } else {
                format!(", {} are not in the index at all", self.unindexed_files)
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProviderAnomaly {
    pub from_file: String,
    pub to_file: String,
    pub message: String,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompileStats {
    pub architecture_node_count: usize,
    pub mapped_file_count: usize,
    pub unassigned_file_count: usize,
    pub ambiguous_file_count: usize,
    /// Rows returned by the provider before filtering and collapsing.
    pub provider_row_count: usize,
    /// Rows dropped by provider.edge_types, exclude_reasons or min_confidence.
    pub filtered_edge_count: usize,
    /// Distinct file-level observations (file pair + relation kind).
    pub observed_edge_count: usize,
    /// Observations whose both files map to architecture nodes.
    pub resolved_edge_count: usize,
    /// Observations with an endpoint deliberately outside the project scope.
    #[serde(default)]
    pub out_of_scope_edge_count: usize,
    #[serde(default)]
    pub unindexed_file_count: usize,
    /// Rows ArchGraph extracted from stylesheets and scripts (`provider.css`),
    /// before filtering; not included in `provider_row_count`.
    #[serde(default)]
    pub stylesheet_row_count: usize,
    /// `FETCHES` rows ArchGraph matched from client HTTP calls to routes
    /// (`provider.http`), before filtering; not included in `provider_row_count`.
    #[serde(default)]
    pub http_row_count: usize,
    /// `IMPORTS` rows from files to imported packages (`provider.packages`),
    /// before filtering; not included in `provider_row_count`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub package_row_count: usize,
    pub aggregated_architecture_edge_count: usize,
    pub violation_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitectureIr {
    pub schema_version: u32,
    pub project: ProjectConfig,
    pub provider: ProviderInfo,
    pub nodes: BTreeMap<String, CompiledNode>,
    pub files: Vec<CompiledFile>,
    pub resolved_edges: Vec<ResolvedEdge>,
    pub edges: Vec<CompiledEdge>,
    pub rules: Vec<RuleConfig>,
    pub violations: Vec<Violation>,
    pub diagnostics: Diagnostics,
    pub stats: CompileStats,
    pub evidence_notice: String,
    /// Present when `provider.css` is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<CssReport>,
    /// Present when `provider.http` is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpReport>,
    /// Present when `provider.packages` is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packages: Option<PackageReport>,
}

/// Stylesheet facts ArchGraph reads itself, since GitNexus does not parse
/// CSS: where each class is defined, where it is used, and which class
/// expressions could not be resolved statically.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CssReport {
    pub stylesheets: Vec<Stylesheet>,
    pub classes: Vec<CssClass>,
    /// Class expressions with a runtime part, e.g. `` `em-${status}` ``.
    pub dynamic_uses: Vec<DynamicClassUse>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stylesheet {
    /// Repository path, or the import specifier of a package stylesheet.
    pub path: String,
    /// Imported from a package (e.g. `leaflet/dist/leaflet.css`); its classes
    /// count as defined, but it is not part of the architecture.
    pub external: bool,
    pub class_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CssClass {
    pub name: String,
    pub status: ClassStatus,
    /// Definitions in repository stylesheets.
    pub definitions: Vec<SourceLine>,
    /// Also (or only) defined by a package stylesheet.
    pub external: bool,
    pub uses: Vec<ClassUse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassStatus {
    Used,
    /// Used, but no stylesheet defines it: a typo or a leftover.
    Undefined,
    /// Defined, never used, and no dynamic expression could produce it.
    Unused,
    /// Defined and never used literally, but a dynamic expression with a
    /// matching prefix may produce it.
    PossiblyDynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceLine {
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClassUse {
    pub file: String,
    pub line: usize,
    pub certainty: ClassCertainty,
}

/// How a class name was found in a script, from most to least certain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassCertainty {
    /// A complete string in a `className`/`class` attribute or property.
    Literal,
    /// A string inside an expression: a branch of `?:`, an array element, a
    /// static part of a template string, a local constant.
    Expression,
    /// A `class="..."` attribute inside an HTML string.
    Markup,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DynamicClassUse {
    pub file: String,
    pub line: usize,
    pub expression: String,
    /// Static text before the runtime part (`em-` in `em-${status}`);
    /// `None` when nothing about the class name is known.
    pub prefix: Option<String>,
}

/// An HTTP endpoint as the code graph provider reports it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Route {
    /// Upper case; `None` when the route accepts any method.
    pub method: Option<String>,
    /// As declared, e.g. `/api/plans/{plan_id}`.
    pub path: String,
    /// The file handling it.
    pub file: String,
}

/// Client HTTP calls ArchGraph reads itself and matches to the provider's
/// routes: which route each call reaches, and which calls reach none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HttpReport {
    pub routes: Vec<RouteUse>,
    /// Calls that reach no route, or reach it with another method.
    pub unmatched: Vec<ClientCall>,
    /// Calls whose URL could not be read statically.
    pub unresolved: Vec<UnresolvedCall>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteUse {
    #[serde(flatten)]
    pub route: Route,
    pub callers: Vec<SourceLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClientCall {
    pub file: String,
    pub line: usize,
    pub method: Option<String>,
    /// With `{}` for the parts known only at runtime.
    pub url: String,
    pub problem: CallProblem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallProblem {
    /// No route has this path: a typo, or a route that was removed.
    NoRoute,
    /// Routes have this path, but none accepts the method.
    WrongMethod,
    /// An absolute URL no route matches, presumably another service.
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UnresolvedCall {
    pub file: String,
    pub line: usize,
    pub expression: String,
}

/// What an imported package's pseudo-path starts with: the `to_file` of a
/// package import is `package:<ecosystem>/<name>`, e.g.
/// `package:python/ortools`, and node `maps` globs match it.
pub const PACKAGE_PREFIX: &str = "package:";

/// The node holding every imported package no node maps. ArchGraph adds it
/// when `provider.packages` is on and the configuration does not define it.
pub const PACKAGES_NODE: &str = "packages";

/// Where a package name comes from; the same name in two ecosystems is two
/// different libraries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    /// A top-level Python module outside the repository and the standard library.
    Python,
    /// A JavaScript/TypeScript package, as named in `package.json`.
    Npm,
}

impl Ecosystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Npm => "npm",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Python => "Python",
            Self::Npm => "npm",
        }
    }
}

pub fn package_id(ecosystem: Ecosystem, name: &str) -> String {
    format!("{PACKAGE_PREFIX}{}/{name}", ecosystem.as_str())
}

/// Third-party packages the repository imports, which GitNexus drops
/// (docs/gitnexus-limitations.md), read by ArchGraph itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PackageReport {
    pub packages: Vec<PackageUse>,
    /// Imports that may name a repository module or a package; not observed.
    pub ambiguous: Vec<AmbiguousImport>,
    /// Dynamic imports whose module is computed at runtime.
    pub unresolved: Vec<UnresolvedImport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageUse {
    /// `package:python/ortools`, `package:npm/@tanstack/react-query`.
    pub id: String,
    pub name: String,
    pub ecosystem: Ecosystem,
    /// The node owning the package: the deepest node whose `maps` match
    /// `id`, else `packages`. `None` when unrelated nodes match it
    /// (`ambiguous_mapping: warn`).
    pub node: Option<String>,
    pub imports: Vec<PackageImport>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PackageImport {
    pub file: String,
    pub line: usize,
    /// As written: `ortools.constraint_solver`, `leaflet/dist/leaflet.css`.
    pub specifier: String,
    /// `import type`, `export type`, or inside `if TYPE_CHECKING:`.
    pub type_only: bool,
    /// The node owning the importing file, if any.
    pub node: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AmbiguousImport {
    pub file: String,
    pub line: usize,
    pub specifier: String,
    /// Why it is neither clearly local nor clearly a package.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UnresolvedImport {
    pub file: String,
    pub line: usize,
    pub expression: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence(i: usize) -> EdgeEvidence {
        EdgeEvidence {
            from_file: format!("src/{i:03}.rs"),
            to_file: "src/end.rs".into(),
            kind: "IMPORTS".into(),
            confidence: Some(i as f64 / 100.0),
            reason: None,
        }
    }
    #[test]
    fn evidence_is_sorted_capped_and_counts_all() {
        let mut a = EvidenceAccumulator::default();
        let mut b = EvidenceAccumulator::default();
        for i in (0..70).rev() {
            a.add(&evidence(i), 20);
        }
        for i in 0..70 {
            b.add(&evidence(i), 20);
        }
        assert_eq!(a.count, 70);
        assert_eq!(a.evidence, b.evidence);
        assert_eq!(a.evidence.len(), 20);
        assert_eq!(a.evidence[19].from_file, "src/019.rs");
        assert_eq!(a.confidence_max, Some(0.69));
    }
    #[test]
    fn aggregates_by_architecture_pair_and_kind() {
        let edges: Vec<_> = (0..3)
            .map(|i| ResolvedEdge {
                from: "app.a".into(),
                to: "app.b".into(),
                kind: "IMPORTS".into(),
                evidence: evidence(i),
            })
            .collect();
        let result = aggregate_observed(&edges, 20);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].count, 3);
        assert_eq!(result[0].evidence.len(), 3);
    }
}
