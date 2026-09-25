use crate::{config::{parent_id, ValidatedConfig}, discovery, mapping, model::*, paths::provider_path, provider::CodeGraphProvider, rules};
use anyhow::{bail, Context, Result};
use std::{collections::{BTreeMap, HashMap}, io::Write, path::{Path, PathBuf}, sync::atomic::{AtomicU64, Ordering}};

/// No process APIs live here. Even reindexing goes through the provider contract.
pub async fn compile(root: &Path, validated: &ValidatedConfig, provider: &dyn CodeGraphProvider, reindex: bool) -> Result<ArchitectureIr> {
    let root = root.canonicalize().context("cannot resolve repository root")?;
    let discovered = discovery::discover(&root, validated)?;
    let mapping::Memberships { files, mut diagnostics } = mapping::resolve(&discovered, validated)?;
    if reindex { provider.reindex().await.context("code-graph reindex failed")?; }
    let provider_info = provider.info().await.context("code-graph provider probe failed")?;
    let observed = provider.import_edges().await.context("code-graph import query failed")?;
    let observed_import_count = observed.len();
    let membership: HashMap<&str, Option<&str>> = files.iter().map(|file| (file.path.as_str(), file.node.as_deref())).collect();
    let mut anomalies: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    let mut resolved = Vec::new();
    for mut edge in observed {
        if edge.kind.trim().is_empty() || edge.confidence.is_some_and(|value| !value.is_finite()) {
            bail!("provider returned an empty edge kind or non-finite confidence for `{}` -> `{}`; fix provider compatibility", edge.from_file, edge.to_file);
        }
        let from_path = provider_path(&root, &edge.from_file);
        let to_path = provider_path(&root, &edge.to_file);
        let (from_path, to_path) = match (from_path, to_path) {
            (Ok(from), Ok(to)) => (from, to),
            (from, to) => {
                let detail = [from.err(), to.err()].into_iter().flatten().map(|error| error.to_string()).collect::<Vec<_>>().join("; ");
                *anomalies.entry((edge.from_file, edge.to_file, detail)).or_default() += 1;
                continue;
            }
        };
        let from_node = membership.get(from_path.as_str());
        let to_node = membership.get(to_path.as_str());
        match (from_node, to_node) {
            (Some(Some(from)), Some(Some(to))) => {
                edge.from_file = from_path;
                edge.to_file = to_path;
                resolved.push(ResolvedEdge { from: (*from).into(), to: (*to).into(), kind: edge.kind.clone(), evidence: edge.into() });
            }
            _ => {
                let message = if from_node.is_none() || to_node.is_none() {
                    "endpoint is outside the discovered project file set; check source_roots, excludes and index freshness"
                } else {
                    "endpoint is unassigned or ambiguously mapped; add unambiguous node maps"
                };
                *anomalies.entry((from_path, to_path, message.into())).or_default() += 1;
            }
        }
    }
    resolved.sort_by(|a, b| evidence_cmp(&a.evidence, &b.evidence)
        .then_with(|| (&a.from, &a.to, &a.kind).cmp(&(&b.from, &b.to, &b.kind))));
    diagnostics.provider_anomalies = anomalies.into_iter().map(|((from_file, to_file, message), count)| ProviderAnomaly { from_file, to_file, message, count }).collect();
    if !diagnostics.provider_anomalies.is_empty() {
        let count: usize = diagnostics.provider_anomalies.iter().map(|a| a.count).sum();
        diagnostics.warnings.push(format!("{count} provider edge(s) could not be mapped; inspect diagnostics.provider_anomalies in the IR"));
    }
    diagnostics.warnings.sort();
    let config = &validated.config;
    let mut nodes: BTreeMap<String, CompiledNode> = config.nodes.iter().map(|(id, node)| (id.clone(), CompiledNode {
        id: id.clone(), title: node.title.clone().unwrap_or_else(|| id.rsplit('.').next().unwrap_or(id).into()),
        kind: node.kind, description: node.description.clone(), parent: parent_id(id).map(str::to_owned),
        children: Vec::new(), direct_files: Vec::new(), descendant_file_count: 0, interfaces: node.interfaces.clone(),
    })).collect();
    for id in config.nodes.keys() {
        if let Some(parent) = parent_id(id) {
            nodes.get_mut(parent).context("validated parent missing while compiling hierarchy")?.children.push(id.clone());
        }
    }
    for file in &files {
        if let Some(owner) = &file.node {
            nodes.get_mut(owner).context("mapped node missing while compiling hierarchy")?.direct_files.push(file.path.clone());
            let mut current = Some(owner.as_str());
            while let Some(id) = current {
                nodes.get_mut(id).context("ancestor missing while counting descendant files")?.descendant_file_count += 1;
                current = parent_id(id);
            }
        }
    }
    let mut edges = aggregate_observed(&resolved, EVIDENCE_LIMIT);
    let mut manual: BTreeMap<(String, String, String), Vec<crate::config::ManualEdgeConfig>> = BTreeMap::new();
    for edge in &config.edges { manual.entry((edge.from.clone(), edge.to.clone(), edge.kind.clone())).or_default().push(edge.clone()); }
    for ((from, to, kind), manual_edges) in manual {
        edges.push(CompiledEdge { from, to, kind, origin: EdgeOrigin::Manual, count: manual_edges.len(),
            evidence: Vec::new(), confidence_min: None, confidence_max: None, manual_edges });
    }
    edges.sort_by(|a, b| (&a.from, &a.to, &a.kind, a.origin).cmp(&(&b.from, &b.to, &b.kind, b.origin)));
    let violations = rules::evaluate(&config.rules, &resolved, EVIDENCE_LIMIT);
    let stats = CompileStats {
        architecture_node_count: nodes.len(), mapped_file_count: files.iter().filter(|file| file.node.is_some()).count(),
        unassigned_file_count: diagnostics.unassigned_files.len(), ambiguous_file_count: diagnostics.ambiguous_files.len(),
        observed_import_count, resolved_import_count: resolved.len(), aggregated_architecture_edge_count: edges.len(),
        violation_count: violations.len(),
    };
    Ok(ArchitectureIr { schema_version: 1, project: config.project.clone(), provider: provider_info, nodes, files,
        resolved_edges: resolved, edges, rules: config.rules.clone(), violations, diagnostics, stats, evidence_notice: EVIDENCE_NOTICE.into() })
}

/// Serialize before replacing the old artifact. A compile/provider failure never
/// writes a new 'clean' report. Readers see an entire JSON file, not a partial write.
pub fn persist(root: &Path, ir: &ArchitectureIr) -> Result<PathBuf> {
    let directory = root.join(".archgraph");
    std::fs::create_dir_all(&directory).with_context(|| format!("cannot create {}", directory.display()))?;
    let destination = directory.join("architecture.ir.json");
    let mut bytes = serde_json::to_vec_pretty(ir).context("cannot serialize architecture IR")?;
    bytes.push(b'\n');
    static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!("architecture.ir.{}.{sequence}.tmp", std::process::id()));
    let mut created = false;
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)
            .with_context(|| format!("cannot create {}; remove a stale temporary file after checking no compile is running", temporary.display()))?;
        created = true;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        // Windows rename cannot overwrite an existing file. Remove only after
        // complete serialization/write; a subsequent rename failure still errors.
        #[cfg(windows)]
        if destination.exists() { std::fs::remove_file(&destination)?; }
        std::fs::rename(&temporary, &destination)?;
        Ok(())
    })();
    if result.is_err() && created { let _ = std::fs::remove_file(&temporary); }
    result.with_context(|| format!("failed to persist {}", destination.display()))?;
    Ok(destination)
}
