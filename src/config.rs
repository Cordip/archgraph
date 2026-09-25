use crate::paths::normalize_relative;
use anyhow::{bail, Context, Result};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, path::Path};

fn default_command() -> String { "gitnexus".into() }
fn default_page_size() -> usize { 1000 }
fn yes() -> bool { true }
fn imports() -> Vec<String> { vec!["IMPORTS".into()] }
fn source_roots() -> Vec<String> { vec![".".into()] }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchitectureConfig {
    pub version: u32,
    pub project: ProjectConfig,
    pub provider: ProviderConfig,
    #[serde(default)]
    pub policies: Policies,
    pub nodes: BTreeMap<String, NodeConfig>,
    #[serde(default)]
    pub edges: Vec<ManualEdgeConfig>,
    #[serde(default)]
    pub rules: Vec<RuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub name: String,
    pub root: String,
    #[serde(default = "source_roots")]
    pub source_roots: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub kind: String,
    #[serde(default = "default_command")]
    pub command: String,
    pub repo: Option<String>,
    #[serde(default = "default_page_size")]
    pub page_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FilePolicy { Ignore, #[default] Warn, Error }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguityPolicy { Warn, #[default] Error }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Policies {
    #[serde(default)]
    pub unassigned_files: FilePolicy,
    #[serde(default)]
    pub ambiguous_mapping: AmbiguityPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind { #[default] Internal, External }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    pub title: Option<String>,
    #[serde(default)]
    pub kind: NodeKind,
    pub description: Option<String>,
    #[serde(default)]
    pub maps: Vec<String>,
    #[serde(default)]
    pub interfaces: Vec<Interface>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Interface {
    pub name: String,
    pub kind: String,
    pub direction: Option<String>,
    pub protocol: Option<String>,
    pub contract: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualEdgeConfig {
    pub id: String,
    pub from: String,
    pub to: String,
    pub kind: String,
    pub label: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuleConfig {
    DenyDependency {
        id: String,
        from: String,
        to: String,
        #[serde(default = "imports")]
        edge_types: Vec<String>,
        #[serde(default = "yes")]
        include_descendants: bool,
    },
    AllowOnly {
        id: String,
        from: String,
        to: Vec<String>,
        #[serde(default = "imports")]
        edge_types: Vec<String>,
        #[serde(default = "yes")]
        include_descendants: bool,
    },
    NoCycles {
        id: String,
        within: String,
        #[serde(default = "imports")]
        edge_types: Vec<String>,
    },
}

impl RuleConfig {
    pub fn id(&self) -> &str {
        match self {
            Self::DenyDependency { id, .. } | Self::AllowOnly { id, .. } | Self::NoCycles { id, .. } => id,
        }
    }
    pub fn kind(&self) -> &str {
        match self {
            Self::DenyDependency { .. } => "deny_dependency",
            Self::AllowOnly { .. } => "allow_only",
            Self::NoCycles { .. } => "no_cycles",
        }
    }
    pub fn references(&self) -> Vec<&str> {
        match self {
            Self::DenyDependency { from, to, .. } => vec![from, to],
            Self::AllowOnly { from, to, .. } => {
                let mut refs = vec![from.as_str()];
                refs.extend(to.iter().map(String::as_str));
                refs
            }
            Self::NoCycles { within, .. } => vec![within],
        }
    }
    pub fn edge_types(&self) -> &[String] {
        match self {
            Self::DenyDependency { edge_types, .. } | Self::AllowOnly { edge_types, .. }
            | Self::NoCycles { edge_types, .. } => edge_types,
        }
    }
}

pub fn valid_node_id(id: &str) -> bool {
    id.split('.').all(|segment| !segment.is_empty()
        && segment.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'))
}
pub fn parent_id(id: &str) -> Option<&str> { id.rsplit_once('.').map(|(parent, _)| parent) }
pub fn is_within(node: &str, ancestor: &str) -> bool {
    node == ancestor || node.strip_prefix(ancestor).is_some_and(|rest| rest.starts_with('.'))
}
pub fn overlaps(a: &str, b: &str) -> bool { is_within(a, b) || is_within(b, a) }
pub fn immediate_child<'a>(node: &'a str, focus: &str) -> Option<&'a str> {
    if node == focus || !is_within(node, focus) { return None; }
    let rest = &node[focus.len() + 1..];
    let segment_len = rest.find('.').unwrap_or(rest.len());
    Some(&node[..focus.len() + 1 + segment_len])
}

/// Validated config owns all compiled globs; neither discovery nor mapping recompiles them.
#[derive(Debug)]
pub struct ValidatedConfig {
    pub config: ArchitectureConfig,
    pub mapping_globs: GlobSet,
    pub mapping_owners: Vec<String>,
    pub exclude_globs: GlobSet,
}

fn add_glob(builder: &mut GlobSetBuilder, raw: &str, owner: &str) -> Result<()> {
    let pattern = raw.replace('\\', "/");
    if pattern.starts_with('/') || pattern.as_bytes().get(1) == Some(&b':')
        || pattern.split('/').any(|p| p == "..") {
        bail!("{owner}: glob `{raw}` must be repository-relative and must not contain `..`");
    }
    if pattern.is_empty() { bail!("{owner}: empty glob is not allowed"); }
    let glob = GlobBuilder::new(&pattern).literal_separator(true).backslash_escape(false)
        .build().with_context(|| format!("{owner}: invalid glob `{raw}`"))?;
    builder.add(glob);
    Ok(())
}

pub fn parse(source: &str) -> Result<ValidatedConfig> {
    let config: ArchitectureConfig = serde_yaml::from_str(source)
        .context("invalid architecture YAML; check field names, indentation and schema version 1")?;
    validate(config)
}

pub fn load(path: &Path) -> Result<ValidatedConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {}; run `archgraph init` or pass --config", path.display()))?;
    parse(&text).with_context(|| path.display().to_string())
}

pub fn validate(mut config: ArchitectureConfig) -> Result<ValidatedConfig> {
    if config.version != 1 { bail!("unsupported architecture version {}; expected 1", config.version); }
    if config.project.name.trim().is_empty() { bail!("project.name must not be empty"); }
    if config.provider.kind != "gitnexus" { bail!("unsupported provider.kind `{}`; MVP supports gitnexus", config.provider.kind); }
    if config.provider.command.trim().is_empty() { bail!("provider.command must not be empty; use gitnexus or an executable path"); }
    if config.provider.repo.as_ref().is_some_and(|repo| repo.trim().is_empty()) {
        bail!("provider.repo must be a nonempty repository name/path or null");
    }
    if config.provider.page_size == 0 { bail!("provider.page_size must be greater than zero"); }
    if config.project.source_roots.is_empty() { bail!("project.source_roots must contain at least one path; use ['.'] for repository root"); }
    for path in &mut config.project.source_roots {
        *path = normalize_relative(path).with_context(|| format!("invalid project.source_roots entry `{path}`"))?;
    }
    config.project.source_roots.sort();
    config.project.source_roots.dedup();
    let root = config.nodes.get(&config.project.root)
        .with_context(|| format!("project.root references missing node `{}`", config.project.root))?;
    if root.kind != NodeKind::Internal { bail!("project.root must reference an internal architecture node"); }
    let mut mappings = GlobSetBuilder::new();
    let mut owners = Vec::new();
    for (id, node) in &config.nodes {
        if !valid_node_id(id) { bail!("invalid node ID `{id}`; use dot-separated ASCII letters, digits, underscores or hyphens"); }
        if let Some(parent) = parent_id(id) {
            let p = config.nodes.get(parent).with_context(|| format!("node `{id}` references missing parent `{parent}`"))?;
            if node.kind == NodeKind::Internal && p.kind == NodeKind::External {
                bail!("internal node `{id}` cannot have external parent `{parent}`");
            }
        }
        if node.kind == NodeKind::External && !node.maps.is_empty() {
            bail!("external node `{id}` cannot own source files; remove its maps");
        }
        for interface in &node.interfaces {
            if interface.name.trim().is_empty() || interface.kind.trim().is_empty() {
                bail!("node `{id}`: interface name and kind must not be empty");
            }
        }
        for pattern in &node.maps {
            add_glob(&mut mappings, pattern, &format!("node `{id}`"))?;
            owners.push(id.clone());
        }
    }
    let mut exclusions = GlobSetBuilder::new();
    for pattern in &config.project.exclude { add_glob(&mut exclusions, pattern, "project.exclude")?; }
    let mut edge_ids = BTreeSet::new();
    for edge in &config.edges {
        if edge.id.trim().is_empty() || !edge_ids.insert(&edge.id) { bail!("manual edge IDs must be nonempty and unique: `{}`", edge.id); }
        if edge.kind.trim().is_empty() { bail!("manual edge `{}` requires a nonempty kind", edge.id); }
        for endpoint in [&edge.from, &edge.to] {
            if !config.nodes.contains_key(endpoint) { bail!("manual edge `{}` references missing node `{endpoint}`", edge.id); }
        }
    }
    let mut rule_ids = BTreeSet::new();
    for rule in &config.rules {
        if rule.id().trim().is_empty() || !rule_ids.insert(rule.id()) { bail!("rule IDs must be nonempty and unique: `{}`", rule.id()); }
        if rule.edge_types().is_empty() || rule.edge_types().iter().any(|t| t.trim().is_empty()) {
            bail!("rule `{}`: edge_types must contain at least one nonempty edge type", rule.id());
        }
        for reference in rule.references() {
            if !config.nodes.contains_key(reference) { bail!("rule `{}` references missing node `{reference}`", rule.id()); }
        }
    }
    config.edges.sort_by(|a, b| a.id.cmp(&b.id));
    config.rules.sort_by(|a, b| a.id().cmp(b.id()));
    Ok(ValidatedConfig { config, mapping_globs: mappings.build()?, mapping_owners: owners, exclude_globs: exclusions.build()? })
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE: &str = "version: 1\nproject: {name: test, root: app}\nprovider: {kind: gitnexus}\nnodes:\n  app: {}\n";
    #[test]
    fn ids_and_segment_boundaries() {
        for id in ["app", "app.foo_1.bar-baz", "A.2"] { assert!(valid_node_id(id)); }
        for id in ["", ".app", "app.", "app..x", "app x", "app/one", "äpp"] { assert!(!valid_node_id(id)); }
        assert!(is_within("app.foo.bar", "app.foo"));
        assert!(!is_within("app.foobar", "app.foo"));
        assert_eq!(immediate_child("app.foo.bar", "app"), Some("app.foo"));
    }
    #[test]
    fn missing_parent_is_actionable() {
        let err = parse(&format!("{BASE}  app.billing.domain: {{}}\n")).unwrap_err();
        assert!(err.to_string().contains("missing parent `app.billing`"));
    }
    #[test]
    fn external_cannot_map() {
        assert!(parse(&format!("{BASE}  external:\n    kind: external\n    maps: ['src/**']\n")).is_err());
    }
    #[test]
    fn references_version_page_size_and_globs_are_validated() {
        assert!(parse(&BASE.replace("version: 1", "version: 2")).is_err());
        assert!(parse(&BASE.replace("{kind: gitnexus}", "{kind: gitnexus, page_size: 0}")).is_err());
        assert!(parse(&format!("{BASE}rules:\n- {{id: x, kind: no_cycles, within: missing}}\n")).is_err());
        assert!(parse(&format!("{BASE}edges:\n- {{id: x, from: app, to: missing, kind: http}}\n")).is_err());
        assert!(parse(&format!("{BASE}  app.bad: {{maps: ['[']}}\n")).is_err());
        assert!(parse(&BASE.replace("kind: gitnexus", "kind: gitnexus, pag_size: 4")).is_err());
    }
}
