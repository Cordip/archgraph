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

/// One summary line per kind of class problem, pointing to `archgraph styles`.
fn css_warnings(report: &CssReport) -> Vec<String> {
    let count = |status| {
        report
            .classes
            .iter()
            .filter(|class| class.status == status)
            .count()
    };
    let unresolved = report
        .dynamic_uses
        .iter()
        .filter(|dynamic| dynamic.prefix.is_none())
        .count();
    [
        (count(ClassStatus::Undefined), "CSS class(es) are used but defined in no stylesheet"),
        (count(ClassStatus::Unused), "CSS class(es) are defined but never used"),
        (unresolved, "class expression(s) could not be resolved statically, so some classes reported unused may be used"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, what)| format!("{count} {what}; see `archgraph styles`"))
    .collect()
}

/// One summary line per kind of import ArchGraph could not attribute,
/// pointing to `archgraph packages`.
fn package_warnings(report: &PackageReport) -> Vec<String> {
    let mut warnings = Vec::new();
    if !report.ambiguous.is_empty() {
        warnings.push(format!(
            "{} import(s) may name a repository module or a package, so they are not observed; see `archgraph packages`",
            report.ambiguous.len()
        ));
    }
    if !report.unresolved.is_empty() {
        warnings.push(format!(
            "{} dynamic import(s) load a module computed at runtime, so their packages are not observed; see `archgraph packages`",
            report.unresolved.len()
        ));
    }
    warnings
}

/// Packages the discovered files import (`provider.packages`), each with the
/// node owning it and every import with the node owning the importing file,
/// and the `IMPORTS` edges to them.
fn imported_packages(
    root: &Path,
    validated: &ValidatedConfig,
    discovered: &[String],
    files: &[CompiledFile],
    provided: &[CodeEdge],
    diagnostics: &mut Diagnostics,
) -> Result<(PackageReport, Vec<CodeEdge>)> {
    if let Some(path) = discovered
        .iter()
        .find(|path| path.starts_with(PACKAGE_PREFIX))
    {
        bail!("file `{path}` would be taken for an imported package (`{PACKAGE_PREFIX}...`) with provider.packages on; rename it or exclude it");
    }
    let extraction = crate::provider::packages::extract(root, discovered, provided)
        .context("cannot read imported packages (provider.packages)")?;
    diagnostics.warnings.extend(extraction.warnings);
    diagnostics
        .warnings
        .extend(package_warnings(&extraction.report));
    let mut report = extraction.report;
    let mut matched = vec![false; validated.package_patterns.len()];
    let owners: HashMap<&str, Option<&str>> = files
        .iter()
        .map(|file| (file.path.as_str(), file.node.as_deref()))
        .collect();
    for package in &mut report.packages {
        for index in validated.package_globs.matches(&package.id) {
            matched[index] = true;
        }
        package.node = mapping::package_owner(&package.id, validated, diagnostics)?;
        for import in &mut package.imports {
            import.node = owners
                .get(import.file.as_str())
                .copied()
                .flatten()
                .map(str::to_owned);
        }
    }
    for (index, _) in matched.iter().enumerate().filter(|(_, hit)| !**hit) {
        diagnostics.warnings.push(format!(
            "node `{}` maps `{}`, which matches no imported package; check the name and ecosystem in `archgraph packages`",
            validated.package_owners[index], validated.package_patterns[index]
        ));
    }
    Ok((report, extraction.edges))
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

/// One summary line per kind of HTTP call problem, pointing to `archgraph http`.
fn http_warnings(report: &HttpReport) -> Vec<String> {
    let mut warnings = Vec::new();
    if report.routes.is_empty() {
        warnings.push("the code-graph provider reported no HTTP routes, so no client call can reach one; see `archgraph http`".to_owned());
    }
    let broken = report
        .unmatched
        .iter()
        .filter(|call| call.problem != CallProblem::External)
        .count();
    if broken > 0 {
        warnings.push(format!(
            "{broken} HTTP call(s) reach no route or use a method the route does not accept; see `archgraph http`"
        ));
    }
    if !report.unresolved.is_empty() {
        warnings.push(format!(
            "{} HTTP call(s) have a URL that could not be read statically, so their dependencies are not observed; see `archgraph http`",
            report.unresolved.len()
        ));
    }
    warnings
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
        let routes = provider
            .routes()
            .await
            .context("code-graph route query failed")?;
        anyhow::Ok((info, edges, indexed_files, routes))
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
    let (info, edges, indexed_files, routes) = match queried {
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
        routes,
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
        mut files,
        mut diagnostics,
    } = mapping::resolve(&discovered, validated)?;
    let (
        Snapshot {
            info: provider_info,
            edges: mut provided,
            indexed_files: indexed_paths,
            routes,
        },
        cached,
    ) = observe(provider, options).await?;
    let provider_row_count = provided.len();
    let css = if validated.config.provider.css {
        let extraction = crate::provider::css::extract(&root, &discovered)
            .context("cannot read stylesheets or class names (provider.css)")?;
        diagnostics.warnings.extend(extraction.warnings);
        Some((extraction.edges, extraction.report))
    } else {
        None
    };
    let stylesheet_row_count = css.as_ref().map_or(0, |(edges, _)| edges.len());
    let css_report = css.map(|(edges, report)| {
        provided.extend(edges);
        report
    });
    if let Some(report) = &css_report {
        diagnostics.warnings.extend(css_warnings(report));
    }
    let http = if validated.config.provider.http {
        let routes = routes.context(
            "provider.http needs HTTP routes, but the code-graph provider cannot list them",
        )?;
        let extraction = crate::provider::http::extract(&root, &discovered, &routes)
            .context("cannot read HTTP calls (provider.http)")?;
        diagnostics.warnings.extend(extraction.warnings);
        diagnostics
            .warnings
            .extend(http_warnings(&extraction.report));
        Some((extraction.edges, extraction.report))
    } else {
        None
    };
    let http_row_count = http.as_ref().map_or(0, |(edges, _)| edges.len());
    let http_report = http.map(|(edges, report)| {
        provided.extend(edges);
        report
    });
    let packages_on = validated.config.provider.packages;
    let (package_report, package_row_count) = if packages_on {
        let (report, edges) = imported_packages(
            &root,
            validated,
            &discovered,
            &files,
            &provided,
            &mut diagnostics,
        )?;
        let rows = edges.len();
        provided.extend(edges);
        (Some(report), rows)
    } else {
        (None, 0)
    };
    let (observed, filtered_edge_count) =
        select_observations(provided, &validated.config.provider)?;
    let observed_edge_count = observed.len();
    let mut membership: HashMap<&str, Option<&str>> = files
        .iter()
        .map(|file| (file.path.as_str(), file.node.as_deref()))
        .collect();
    if let Some(report) = &package_report {
        membership.extend(
            report
                .packages
                .iter()
                .map(|package| (package.id.as_str(), package.node.as_deref())),
        );
    }
    // A package pseudo-path is not a file path; nothing else may start so.
    let is_package = |path: &str| packages_on && path.starts_with(PACKAGE_PREFIX);
    let mut anomalies: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    let mut resolved = Vec::new();
    // Edges to files the configuration deliberately leaves out (excluded,
    // outside source_roots, or unassigned under `unassigned_files: ignore`)
    // are expected; they are counted, not reported as anomalies.
    let ignore_unassigned =
        validated.config.policies.unassigned_files == crate::config::FilePolicy::Ignore;
    let out_of_scope = |path: &str, owner: Option<&Option<&str>>| match owner {
        _ if is_package(path) => false,
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
    // (used file, user file): every observed dependency on a mapped file
    // from another file, whatever the user's scope. An excluded script
    // importing a module still uses it.
    let mut uses: BTreeSet<(String, String)> = BTreeSet::new();
    for mut edge in observed {
        let from_path = provider_path(&root, &edge.from_file);
        let to_path = if is_package(&edge.to_file) {
            Ok(edge.to_file.clone())
        } else {
            provider_path(&root, &edge.to_file)
        };
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
        if matches!(to_node, Some(Some(_))) && !is_package(&to_path) && from_path != to_path {
            uses.insert((to_path.clone(), from_path.clone()));
        }
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
                    packages: Vec::new(),
                    descendant_package_count: 0,
                    entry_point_count: 0,
                    no_observed_users_count: 0,
                    outside_user_count: 0,
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
    // Coverage asks whether the provider sees a file's dependencies on other
    // repository files; an import of a package says nothing about that.
    let observed_files: HashSet<&str> = resolved
        .iter()
        .filter(|edge| !is_package(&edge.evidence.to_file))
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
    let used: HashSet<&str> = uses.iter().map(|(used, _)| used.as_str()).collect();
    let mut entry_matched = vec![false; config.project.entry_points.len()];
    let mut usages = Vec::with_capacity(files.len());
    for file in &files {
        let mut usage = None;
        if let Some(owner) = &file.node {
            let observed = observed_files.contains(file.path.as_str());
            let unindexed = indexed
                .as_ref()
                .is_some_and(|indexed| !indexed.contains(&file.path));
            if unindexed {
                diagnostics.unindexed_files.push(file.path.clone());
            }
            let entry_points = validated.entry_globs.matches(&file.path);
            for &index in &entry_points {
                entry_matched[index] = true;
            }
            // A user seen beats "not indexed": ArchGraph's own CSS and HTTP
            // edges reach files GitNexus does not index.
            let status = if !entry_points.is_empty() {
                FileUsage::EntryPoint
            } else if used.contains(file.path.as_str()) {
                FileUsage::Used
            } else if unindexed {
                FileUsage::NotIndexed
            } else {
                FileUsage::NoObservedUsers
            };
            usage = Some(status);
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
                node.entry_point_count += usize::from(status == FileUsage::EntryPoint);
                node.no_observed_users_count += usize::from(status == FileUsage::NoObservedUsers);
                current = parent_id(id);
            }
        }
        usages.push(usage);
    }
    for (pattern, _) in config
        .project
        .entry_points
        .iter()
        .zip(&entry_matched)
        .filter(|(_, matched)| !**matched)
    {
        diagnostics.warnings.push(format!(
            "project.entry_points entry `{pattern}` matches no mapped file; check the path, or map the file to a node"
        ));
    }
    // Distinct users outside each subtree: a node everything outside
    // ignores may be unused as a whole.
    let mut outside_users: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (used, user) in &uses {
        let Some(Some(owner)) = membership.get(used.as_str()) else {
            continue;
        };
        let user_owner = membership.get(user.as_str()).copied().flatten();
        let mut current = Some(*owner);
        while let Some(id) = current {
            if !user_owner.is_some_and(|user_owner| crate::config::is_within(user_owner, id)) {
                outside_users.entry(id).or_default().insert(user.as_str());
            }
            current = parent_id(id);
        }
    }
    for (id, users) in outside_users {
        nodes
            .get_mut(id)
            .context("used node missing while counting outside users")?
            .outside_user_count = users.len();
    }
    for (file, usage) in files.iter_mut().zip(usages) {
        file.usage = usage;
    }
    if let Some(report) = &package_report {
        for package in &report.packages {
            let Some(owner) = &package.node else {
                continue;
            };
            nodes
                .get_mut(owner)
                .context("package owner missing while compiling hierarchy")?
                .packages
                .push(package.id.clone());
            let mut current = Some(owner.as_str());
            while let Some(id) = current {
                nodes
                    .get_mut(id)
                    .context("ancestor missing while counting packages")?
                    .descendant_package_count += 1;
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
        stylesheet_row_count,
        http_row_count,
        package_row_count,
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
        css: css_report,
        http: http_report,
        packages: package_report,
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
/// `.archgraph/`, holding only generated files. Like GitNexus's `.gitnexus/`,
/// it ignores itself (`.gitignore` with `*`), so the analyzed repository's Git
/// status stays clean. An existing `.gitignore` is left alone.
pub fn work_directory(root: &Path) -> Result<PathBuf> {
    let directory = root.join(".archgraph");
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("cannot create {}", directory.display()))?;
    let ignore = directory.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n")
            .with_context(|| format!("cannot write {}", ignore.display()))?;
    }
    Ok(directory)
}

pub fn persist(root: &Path, ir: &ArchitectureIr) -> Result<PathBuf> {
    let destination = work_directory(root)?.join("architecture.ir.json");
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
                    packages: Vec::new(),
                    descendant_package_count: 0,
                    entry_point_count: 0,
                    no_observed_users_count: 0,
                    outside_user_count: 0,
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
