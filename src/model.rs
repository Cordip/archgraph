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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledFile {
    pub path: String,
    pub node: Option<String>,
    pub ambiguous_matches: Vec<String>,
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
