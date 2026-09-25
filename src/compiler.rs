use crate::{
    cache::{self, Snapshot},
    config::{parent_id, ProviderConfig, ValidatedConfig},
    discovery, mapping,
    model::*,
    paths::provider_path,
    provider::{CodeGraphProvider, ReindexMode},
    rules,
};
use anyhow::{bail, Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Default)]
pub struct CompileOptions {
    pub reindex: Option<ReindexMode>,
    /// File for provider results kept between runs (see `cache`); `None`
    /// always queries the provider.
    pub cache: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Compiled {
    pub ir: ArchitectureIr,
    /// Provider results came from the cache: the index had not changed.
    pub cached: bool,
}

/// No process APIs live here. Even reindexing goes through the provider contract.
pub async fn compile(
    root: &Path,
    validated: &ValidatedConfig,
    provider: &dyn CodeGraphProvider,
    reindex: Option<ReindexMode>,
) -> Result<ArchitectureIr> {
    let options = CompileOptions {
        reindex,
        cache: None,
    };
    Ok(compile_with(root, validated, provider, &options).await?.ir)
}

/// Reads everything the compiler needs from the provider, from the cache when
/// the index is unchanged since it was written.
async fn observe(
    provider: &dyn CodeGraphProvider,
    options: &CompileOptions,
) -> Result<(Snapshot, bool)> {
    if let Some(mode) = options.reindex {
        provider
            .reindex(mode)
            .await
            .context("code-graph reindex failed")?;
    }
    let index_before = provider
        .fingerprint()
        .await
        .context("cannot read the code-graph index identity")?;
    let cache_entry = options.cache.as_deref().and_then(|path| {
        let key = cache::key(
            index_before.as_deref(),
            provider.query_identity().as_deref(),
        )?;
        Some((path, key))
    });
    if let Some((path, key)) = &cache_entry {
        if let Some(snapshot) = cache::load(path, key) {
            return Ok((snapshot, true));
        }
    }
    const INDEX_CHANGED: &str = "the code-graph index changed while compiling (another `gitnexus analyze` ran, e.g. an auto-index service); rerun when indexing has finished";
    let queried = async {
        let info = provider
            .info()
            .await
            .context("code-graph provider probe failed")?;
        let edges = provider
            .dependency_edges()
            .await
            .context("code-graph dependency query failed")?;
        let indexed_files = provider
            .indexed_files()
            .await
            .context("code-graph file query failed")?;
        anyhow::Ok((info, edges, indexed_files))
    }
    .await;
    let index_after = provider
        .fingerprint()
        .await
        .context("cannot read the code-graph index identity")?;
    // Paged queries against an index being rewritten mix two graphs, or read
    // a half-written one; neither result may be reported, clean or not. A
    // query that failed meanwhile failed because of the rewrite, not because
    // GitNexus is incompatible.
    let (info, edges, indexed_files) = match queried {
        Err(error) if index_before != index_after => {
            return Err(error.context(INDEX_CHANGED));
        }
        result => result?,
    };
    if index_before != index_after {
        bail!(INDEX_CHANGED);
    }
    let snapshot = Snapshot {
        info,
        edges,
        indexed_files,
    };
    if let Some((path, key)) = &cache_entry {
        // A cache that cannot be written only costs speed on the next run.
        if let Err(error) = cache::store(path, key, &snapshot) {
            eprintln!("warning: {error:#}");
        }
    }
    Ok((snapshot, false))
}

pub async fn compile_with(
    root: &Path,
    validated: &ValidatedConfig,
    provider: &dyn CodeGraphProvider,
    options: &CompileOptions,
) -> Result<Compiled> {
    let root = root
        .canonicalize()
        .context("cannot resolve repository root")?;
    let discovered = discovery::discover(&root, validated)?;
    let mapping::Memberships {
        files,
        mut diagnostics,
    } = mapping::resolve(&discovered, validated)?;
    let (
        Snapshot {
            info: provider_info,
            edges: provided,
            indexed_files: indexed_paths,
        },
        cached,
    ) = observe(provider, options).await?;
    let provider_row_count = provided.len();
    let (observed, filtered_edge_count) =
        select_observations(provided, &validated.config.provider)?;
    let observed_edge_count = observed.len();
    let membership: HashMap<&str, Option<&str>> = files
        .iter()
        .map(|file| (file.path.as_str(), file.node.as_deref()))
        .collect();
    let mut anomalies: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    let mut resolved = Vec::new();
    // Edges to files the configuration deliberately leaves out (excluded,
    // outside source_roots, or unassigned under `unassigned_files: ignore`)
    // are expected; they are counted, not reported as anomalies.
    let ignore_unassigned =
        validated.config.policies.unassigned_files == crate::config::FilePolicy::Ignore;
    let out_of_scope = |path: &str, owner: Option<&Option<&str>>| match owner {
        None => {
            validated.exclude_globs.is_match(path)
                || !validated
                    .config
                    .project
                    .source_roots
                    .iter()
                    .any(|source| crate::discovery::under(path, source))
        }
        Some(None) => ignore_unassigned && !diagnostics.ambiguous_files.contains_key(path),
        Some(Some(_)) => false,
    };
    let mut out_of_scope_edge_count = 0;
    for mut edge in observed {
        let from_path = provider_path(&root, &edge.from_file);
        let to_path = provider_path(&root, &edge.to_file);
        let (from_path, to_path) = match (from_path, to_path) {
            (Ok(from), Ok(to)) => (from, to),
            (from, to) => {
                let detail = [from.err(), to.err()]
                    .into_iter()
                    .flatten()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                *anomalies
                    .entry((edge.from_file, edge.to_file, detail))
                    .or_default() += 1;
                continue;
            }
        };
        let from_node = membership.get(from_path.as_str());
        let to_node = membership.get(to_path.as_str());
        match (from_node, to_node) {
            (Some(Some(from)), Some(Some(to))) => {
                edge.from_file = from_path;
                edge.to_file = to_path;
                resolved.push(ResolvedEdge {
                    from: (*from).into(),
                    to: (*to).into(),
                    kind: edge.kind.clone(),
                    evidence: edge.into(),
                });
            }
            _ if out_of_scope(&from_path, from_node) || out_of_scope(&to_path, to_node) => {
                out_of_scope_edge_count += 1;
            }
            _ => {
                let message = if from_node.is_none() || to_node.is_none() {
                    "endpoint is in scope but was not discovered: ignored by .gitignore, deleted, or the index is stale"
                } else {
                    "endpoint is unassigned or ambiguously mapped; add unambiguous node maps"
                };
                *anomalies
                    .entry((from_path, to_path, message.into()))
                    .or_default() += 1;
            }
        }
    }
    resolved.sort_by(|a, b| {
        evidence_cmp(&a.evidence, &b.evidence)
            .then_with(|| (&a.from, &a.to, &a.kind).cmp(&(&b.from, &b.to, &b.kind)))
    });
    diagnostics.provider_anomalies = anomalies
        .into_iter()
        .map(|((from_file, to_file, message), count)| ProviderAnomaly {
            from_file,
            to_file,
            message,
            count,
        })
        .collect();
    if !diagnostics.provider_anomalies.is_empty() {
        let count: usize = diagnostics.provider_anomalies.iter().map(|a| a.count).sum();
        diagnostics.warnings.push(format!("{count} provider edge(s) could not be mapped; inspect diagnostics.provider_anomalies in the IR"));
        let fileless: usize = diagnostics
            .provider_anomalies
            .iter()
            .filter(|anomaly| anomaly.from_file.is_empty() || anomaly.to_file.is_empty())
            .map(|anomaly| anomaly.count)
            .sum();
        if fileless > 0 {
            diagnostics.warnings.push(format!("{fileless} provider edge(s) involve symbols stored without a file path, which incremental GitNexus indexing can leave behind; `--reindex=full` rebuilds the index (see docs/gitnexus-limitations.md)"));
        }
    }
    diagnostics.warnings.sort();
    let config = &validated.config;
    let mut nodes: BTreeMap<String, CompiledNode> = config
        .nodes
        .iter()
        .map(|(id, node)| {
            (
                id.clone(),
                CompiledNode {
                    id: id.clone(),
                    title: node
                        .title
                        .clone()
                        .unwrap_or_else(|| id.rsplit('.').next().unwrap_or(id).into()),
                    kind: node.kind,
                    description: node.description.clone(),
                    parent: parent_id(id).map(str::to_owned),
                    children: Vec::new(),
                    direct_files: Vec::new(),
                    descendant_file_count: 0,
                    observed_file_count: 0,
                    unindexed_file_count: 0,
                    interfaces: node.interfaces.clone(),
                },
            )
        })
        .collect();
    for id in config.nodes.keys() {
        if let Some(parent) = parent_id(id) {
            nodes
                .get_mut(parent)
                .context("validated parent missing while compiling hierarchy")?
                .children
                .push(id.clone());
        }
    }
    let observed_files: HashSet<&str> = resolved
        .iter()
        .flat_map(|edge| {
            [
                edge.evidence.from_file.as_str(),
                edge.evidence.to_file.as_str(),
            ]
        })
        .collect();
    let indexed: Option<HashSet<String>> = indexed_paths.map(|paths| {
        paths
            .iter()
            .filter_map(|path| provider_path(&root, path).ok())
            .collect()
    });
    for file in &files {
        if let Some(owner) = &file.node {
            let observed = observed_files.contains(file.path.as_str());
            let unindexed = indexed
                .as_ref()
                .is_some_and(|indexed| !indexed.contains(&file.path));
            if unindexed {
                diagnostics.unindexed_files.push(file.path.clone());
            }
            nodes
                .get_mut(owner)
                .context("mapped node missing while compiling hierarchy")?
                .direct_files
                .push(file.path.clone());
            let mut current = Some(owner.as_str());
            while let Some(id) = current {
                let node = nodes
                    .get_mut(id)
                    .context("ancestor missing while counting descendant files")?;
                node.descendant_file_count += 1;
                node.observed_file_count += usize::from(observed);
                node.unindexed_file_count += usize::from(unindexed);
                current = parent_id(id);
            }
        }
    }
    let mut edges = aggregate_observed(&resolved, EVIDENCE_LIMIT);
    let mut manual: BTreeMap<(String, String, String), Vec<crate::config::ManualEdgeConfig>> =
        BTreeMap::new();
    for edge in &config.edges {
        manual
            .entry((edge.from.clone(), edge.to.clone(), edge.kind.clone()))
            .or_default()
            .push(edge.clone());
    }
    for ((from, to, kind), manual_edges) in manual {
        edges.push(CompiledEdge {
            from,
            to,
            kind,
            origin: EdgeOrigin::Manual,
            count: manual_edges.len(),
            evidence: Vec::new(),
            confidence_min: None,
            confidence_max: None,
            manual_edges,
        });
    }
    edges.sort_by(|a, b| {
        (&a.from, &a.to, &a.kind, a.origin).cmp(&(&b.from, &b.to, &b.kind, b.origin))
    });
    let violations = rules::evaluate(&config.rules, &resolved, EVIDENCE_LIMIT);
    if let Some(example) = diagnostics.unindexed_files.first() {
        diagnostics.warnings.push(format!(
            "{} mapped file(s) are not in the code-graph index at all (e.g. `{example}`) and can never show dependencies; the provider skips some directories and file types (see docs/gitnexus-limitations.md)",
            diagnostics.unindexed_files.len()
        ));
    }
    diagnostics.low_coverage =
        coverage_issues(&config.rules, &nodes, config.policies.min_observed_ratio);
    if config.policies.low_coverage != crate::config::FilePolicy::Ignore {
        diagnostics
            .warnings
            .extend(diagnostics.low_coverage.iter().map(ToString::to_string));
    }
    diagnostics.warnings.sort();
    let stats = CompileStats {
        architecture_node_count: nodes.len(),
        mapped_file_count: files.iter().filter(|file| file.node.is_some()).count(),
        unassigned_file_count: diagnostics.unassigned_files.len(),
        ambiguous_file_count: diagnostics.ambiguous_files.len(),
        provider_row_count,
        filtered_edge_count,
        observed_edge_count,
        resolved_edge_count: resolved.len(),
        out_of_scope_edge_count,
        unindexed_file_count: diagnostics.unindexed_files.len(),
        aggregated_architecture_edge_count: edges.len(),
        violation_count: violations.len(),
    };
    let ir = ArchitectureIr {
        schema_version: 1,
        project: config.project.clone(),
        provider: provider_info,
        nodes,
        files,
        resolved_edges: resolved,
        edges,
        rules: config.rules.clone(),
        violations,
        diagnostics,
        stats,
        evidence_notice: EVIDENCE_NOTICE.into(),
    };
    Ok(Compiled { ir, cached })
}

/// Rule nodes that are mostly invisible to the provider, e.g. an unparsed
/// language or unresolved import aliases.
fn coverage_issues(
    rules: &[crate::config::RuleConfig],
    nodes: &BTreeMap<String, CompiledNode>,
    min_ratio: f64,
) -> Vec<CoverageIssue> {
    let mut issues = BTreeSet::new();
    for rule in rules {
        for reference in rule.references() {
            let Some(node) = nodes.get(reference) else {
                continue;
            };
            let total = node.descendant_file_count;
            if total == 0 || (node.observed_file_count as f64) >= min_ratio * total as f64 {
                continue;
            }
            issues.insert(CoverageIssue {
                rule_id: rule.id().into(),
                node: node.id.clone(),
                observed_files: node.observed_file_count,
                unindexed_files: node.unindexed_file_count,
                total_files: total,
            });
        }
    }
    issues.into_iter().collect()
}

type FilePairKind = (String, String, String);

/// Applies the configured edge-type, reason and confidence filters, then
/// collapses provider rows to one observation per file pair and relation kind
/// (maximum confidence, all surviving reasons). Returns the dropped row count.
pub fn select_observations(
    edges: Vec<CodeEdge>,
    provider: &ProviderConfig,
) -> Result<(Vec<CodeEdge>, usize)> {
    let mut filtered = 0;
    let mut groups: BTreeMap<FilePairKind, (Option<f64>, BTreeSet<String>)> = BTreeMap::new();
    for edge in edges {
        if edge.kind.trim().is_empty() || edge.confidence.is_some_and(|value| !value.is_finite()) {
            bail!("provider returned an empty edge kind or non-finite confidence for `{}` -> `{}`; fix provider compatibility", edge.from_file, edge.to_file);
        }
        if !provider.edge_types.contains(&edge.kind)
            || !provider.accepts(edge.reason.as_deref(), edge.confidence)
        {
            filtered += 1;
            continue;
        }
        let (confidence, reasons) = groups
            .entry((edge.from_file, edge.to_file, edge.kind))
            .or_default();
        if let Some(value) = edge.confidence {
            *confidence = Some(confidence.map_or(value, |current: f64| current.max(value)));
        }
        reasons.extend(edge.reason);
    }
    let observations = groups
        .into_iter()
        .map(
            |((from_file, to_file, kind), (confidence, reasons))| CodeEdge {
                from_file,
                to_file,
                kind,
                confidence,
                reason: (!reasons.is_empty())
                    .then(|| reasons.into_iter().collect::<Vec<_>>().join("; ")),
            },
        )
        .collect();
    Ok((observations, filtered))
}

/// Serialize before replacing the old artifact. A compile/provider failure never
/// writes a new 'clean' report. Readers see an entire JSON file, not a partial write.
pub fn persist(root: &Path, ir: &ArchitectureIr) -> Result<PathBuf> {
    let directory = root.join(".archgraph");
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("cannot create {}", directory.display()))?;
    let destination = directory.join("architecture.ir.json");
    let mut bytes = serde_json::to_vec_pretty(ir).context("cannot serialize architecture IR")?;
    bytes.push(b'\n');
    write_atomically(&destination, &bytes)?;
    Ok(destination)
}

/// Readers never see a partial file: write a sibling temporary, then rename.
pub fn write_atomically(destination: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let name = destination
        .file_stem()
        .context("destination has no file name")?
        .to_string_lossy();
    let temporary =
        destination.with_file_name(format!("{name}.{}.{sequence}.tmp", std::process::id()));
    let mut created = false;
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)
            .with_context(|| format!("cannot create {}; remove a stale temporary file after checking no compile is running", temporary.display()))?;
        created = true;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        // Windows rename cannot overwrite an existing file. Remove only after
        // complete serialization/write; a subsequent rename failure still errors.
        #[cfg(windows)]
        if destination.exists() {
            std::fs::remove_file(destination)?;
        }
        std::fs::rename(&temporary, destination)?;
        Ok(())
    })();
    if result.is_err() && created {
        let _ = std::fs::remove_file(&temporary);
    }
    result.with_context(|| format!("failed to persist {}", destination.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edge(from: &str, to: &str, kind: &str, reason: &str, confidence: f64) -> CodeEdge {
        CodeEdge {
            from_file: from.into(),
            to_file: to.into(),
            kind: kind.into(),
            confidence: Some(confidence),
            reason: Some(reason.into()),
        }
    }
    #[test]
    fn observations_are_filtered_then_collapsed_per_file_pair_and_kind() {
        let config = crate::config::parse(
            "version: 1\nproject: {name: t, root: app}\nprovider: {kind: gitnexus, edge_types: [CALLS, IMPORTS], min_confidence: 0.6}\nnodes: {app: {}}\n",
        ).unwrap();
        let (kept, filtered) = select_observations(
            vec![
                edge("a.rb", "b.rb", "CALLS", "import-resolved", 0.85),
                edge("a.rb", "b.rb", "CALLS", "property-dispatch", 0.7),
                edge("a.rb", "b.rb", "CALLS", "global-name-fallback", 0.5),
                edge("a.rb", "b.rb", "IMPORTS", "ruby-scope: import", 1.0),
                edge("README.md", "a.rb", "IMPORTS", "markdown-link", 0.8),
                edge("a.rb", "c.rb", "ACCESSES", "read", 1.0),
            ],
            &config.config.provider,
        )
        .unwrap();
        assert_eq!(filtered, 3);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].kind, "CALLS");
        assert_eq!(kept[0].confidence, Some(0.85));
        assert_eq!(
            kept[0].reason.as_deref(),
            Some("import-resolved; property-dispatch")
        );
        assert_eq!(kept[1].kind, "IMPORTS");
    }
    #[test]
    fn rules_over_mostly_unobserved_nodes_warn() {
        let config = crate::config::parse(
            "version: 1\nproject: {name: t, root: app}\nprovider: {kind: gitnexus}\nnodes: {app: {}, app.a: {}, app.b: {}}\nrules:\n- {id: r, kind: deny_dependency, from: app.a, to: app.b}\n",
        ).unwrap();
        let node = |id: &str, total, observed| {
            (
                id.to_string(),
                CompiledNode {
                    id: id.into(),
                    title: id.into(),
                    kind: Default::default(),
                    description: None,
                    parent: None,
                    children: Vec::new(),
                    direct_files: Vec::new(),
                    descendant_file_count: total,
                    observed_file_count: observed,
                    unindexed_file_count: (total - observed) / 2,
                    interfaces: Vec::new(),
                },
            )
        };
        let nodes = BTreeMap::from([
            node("app", 20, 11),
            node("app.a", 10, 1),
            node("app.b", 10, 10),
        ]);
        let issues = coverage_issues(&config.config.rules, &nodes, 0.5);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0]
            .to_string()
            .contains("only 1 of 10 files under `app.a`"));
        assert!(issues[0]
            .to_string()
            .contains("4 are not in the index at all"));
        assert_eq!(coverage_issues(&config.config.rules, &nodes, 0.05).len(), 0);
    }
}
