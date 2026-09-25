use crate::{
    config::{is_within, AmbiguityPolicy, FilePolicy, ValidatedConfig},
    model::{CompiledFile, Diagnostics, PACKAGES_NODE},
    paths::normalize_relative,
};
use anyhow::{bail, Result};
use std::collections::BTreeSet;

#[derive(Debug)]
pub struct Memberships {
    pub files: Vec<CompiledFile>,
    pub diagnostics: Diagnostics,
}

pub fn resolve(files: &[String], validated: &ValidatedConfig) -> Result<Memberships> {
    let mut memberships = Vec::new();
    let mut diagnostics = Diagnostics::default();
    let paths: BTreeSet<String> = files
        .iter()
        .map(|path| normalize_relative(path))
        .collect::<Result<_>>()?;
    for path in paths {
        let matches = validated
            .mapping_globs
            .matches(&path)
            .into_iter()
            .map(|index| validated.mapping_owners[index].clone())
            .collect();
        let mut file = CompiledFile {
            path: path.clone(),
            node: None,
            ambiguous_matches: Vec::new(),
            usage: None,
        };
        match owner(&path, "file", matches, validated, &mut diagnostics)? {
            Owner::None => {
                diagnostics.unassigned_files.push(path.clone());
                match validated.config.policies.unassigned_files {
                    FilePolicy::Error => bail!("file `{path}` is unassigned; add a node maps glob or change policies.unassigned_files"),
                    FilePolicy::Warn => {}
                    FilePolicy::Ignore => {}
                }
            }
            Owner::Node(node) => file.node = Some(node),
            Owner::Ambiguous(ranked) => {
                file.ambiguous_matches = ranked.clone();
                diagnostics.ambiguous_files.insert(path, ranked);
            }
        }
        memberships.push(file);
    }
    if !diagnostics.unassigned_files.is_empty()
        && validated.config.policies.unassigned_files == FilePolicy::Warn
    {
        diagnostics.warnings.push(format!(
            "{} unassigned file(s); add maps globs or adjust policies.unassigned_files",
            diagnostics.unassigned_files.len()
        ));
    }
    diagnostics.warnings.sort();
    Ok(Memberships {
        files: memberships,
        diagnostics,
    })
}

enum Owner {
    None,
    Node(String),
    /// Nodes on unrelated branches, sorted; under `ambiguous_mapping: warn`.
    Ambiguous(Vec<String>),
}

/// The node a file or package belongs to among the nodes whose maps match
/// it: the highest priority, then the deepest of one ancestor chain.
/// Unrelated branches fail under `ambiguous_mapping: error`; under `warn`
/// nothing is chosen.
fn owner(
    path: &str,
    what: &str,
    matches: BTreeSet<String>,
    validated: &ValidatedConfig,
    diagnostics: &mut Diagnostics,
) -> Result<Owner> {
    let priority = |id: &String| {
        validated
            .config
            .nodes
            .get(id)
            .map_or(0, |node| node.priority)
    };
    let top = matches.iter().map(priority).max().unwrap_or(0);
    let mut ranked: Vec<String> = matches
        .into_iter()
        .filter(|id| priority(id) == top)
        .collect();
    ranked.sort_by(|a, b| {
        a.split('.')
            .count()
            .cmp(&b.split('.').count())
            .then_with(|| a.cmp(b))
    });
    if ranked.is_empty() {
        return Ok(Owner::None);
    }
    if ranked.windows(2).all(|pair| is_within(&pair[1], &pair[0])) {
        return Ok(Owner::Node(ranked.pop().unwrap_or_default()));
    }
    let unrelated = ranked.iter().enumerate().find_map(|(i, a)| {
        ranked
            .iter()
            .skip(i + 1)
            .find(|b| !is_within(a, b) && !is_within(b, a))
            .map(|b| (a, b))
    });
    if let Some((a, b)) = unrelated {
        let message = format!("{what} `{path}` matches unrelated architecture nodes `{a}` and `{b}`; make maps unambiguous or change policies.ambiguous_mapping");
        if validated.config.policies.ambiguous_mapping == AmbiguityPolicy::Error {
            bail!("{message}");
        }
        diagnostics.warnings.push(message);
    }
    // Warn is never permission to arbitrarily assign an unrelated branch.
    ranked.sort();
    Ok(Owner::Ambiguous(ranked))
}

/// The node owning an imported package (`package:<ecosystem>/<name>`): the
/// node whose `package:` maps match it, else `packages`. `None` when
/// unrelated nodes match it under `ambiguous_mapping: warn`.
pub fn package_owner(
    id: &str,
    validated: &ValidatedConfig,
    diagnostics: &mut Diagnostics,
) -> Result<Option<String>> {
    let matches = validated
        .package_globs
        .matches(id)
        .into_iter()
        .map(|index| validated.package_owners[index].clone())
        .collect();
    Ok(
        match owner(id, "package", matches, validated, diagnostics)? {
            Owner::None => Some(PACKAGES_NODE.into()),
            Owner::Node(node) => Some(node),
            Owner::Ambiguous(_) => None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(nodes: &str, policies: &str) -> ValidatedConfig {
        crate::config::parse(&format!("version: 1\nproject: {{name: t, root: app}}\nprovider: {{kind: gitnexus}}\npolicies: {policies}\nnodes:\n  app: {{maps: ['src/**']}}\n{nodes}")).unwrap()
    }
    #[test]
    fn deepest_match_and_cross_platform_paths() {
        let c = config(
            "  app.foo: {maps: ['src/foo/**']}\n  app.foo.api: {maps: ['src/foo/api/**']}\n",
            "{}",
        );
        let m = resolve(&[r"src\foo\api\a.rs".into(), "src/b.rs".into()], &c).unwrap();
        assert_eq!(m.files[0].node.as_deref(), Some("app"));
        assert_eq!(m.files[1].node.as_deref(), Some("app.foo.api"));
    }
    #[test]
    fn unrelated_branches_error_by_default() {
        let c = config(
            "  app.foo: {maps: ['src/**']}\n  app.foobar: {maps: ['src/**']}\n",
            "{}",
        );
        assert!(resolve(&["src/a.rs".into()], &c)
            .unwrap_err()
            .to_string()
            .contains("unrelated architecture nodes"));
    }
    #[test]
    fn warning_keeps_ambiguity_explicit() {
        let c = config(
            "  app.a: {maps: ['src/**']}\n  app.b: {maps: ['src/**']}\n",
            "{ambiguous_mapping: warn}",
        );
        let m = resolve(&["src/a".into()], &c).unwrap();
        assert!(m.files[0].node.is_none());
        assert_eq!(m.diagnostics.ambiguous_files.len(), 1);
        assert!(m.diagnostics.unassigned_files.is_empty());
    }
    #[test]
    fn unassigned_policies() {
        for policy in ["ignore", "warn", "error"] {
            let c = config("", &format!("{{unassigned_files: {policy}}}"));
            let result = resolve(&["other/a".into()], &c);
            if policy == "error" {
                assert!(result.is_err());
            } else {
                let result = result.unwrap();
                assert_eq!(result.diagnostics.unassigned_files, ["other/a"]);
                assert_eq!(result.diagnostics.warnings.is_empty(), policy == "ignore");
            }
        }
    }
    #[test]
    fn priority_lets_a_cross_cutting_node_claim_co_located_files() {
        let nodes = "  app.web: {maps: ['src/web/**']}\n  app.tests: {maps: ['src/**/__tests__/**'], priority: 1}\n";
        let c = config(nodes, "{}");
        let m = resolve(
            &["src/web/__tests__/a.spec.ts".into(), "src/web/a.ts".into()],
            &c,
        )
        .unwrap();
        assert_eq!(m.files[0].node.as_deref(), Some("app.tests"));
        assert_eq!(m.files[1].node.as_deref(), Some("app.web"));
        let without = config(&nodes.replace(", priority: 1", ""), "{}");
        assert!(resolve(&["src/web/__tests__/a.spec.ts".into()], &without).is_err());
    }
    #[test]
    fn packages_go_to_the_deepest_mapping_node_or_to_packages() {
        let nodes = "  libs: {kind: external, maps: ['package:npm/**']}\n  libs.map: {kind: external, maps: ['package:npm/leaflet']}\n  other: {kind: external, maps: ['package:*/yaml']}\n";
        let yaml = |policy: &str| {
            crate::config::parse(&format!("version: 1\nproject: {{name: t, root: app}}\nprovider: {{kind: gitnexus, packages: true}}\npolicies: {policy}\nnodes:\n  app: {{}}\n{nodes}")).unwrap()
        };
        let c = yaml("{}");
        let mut diagnostics = Diagnostics::default();
        let owner = |id: &str, diagnostics: &mut Diagnostics| package_owner(id, &c, diagnostics);
        assert_eq!(
            owner("package:npm/leaflet", &mut diagnostics)
                .unwrap()
                .as_deref(),
            Some("libs.map")
        );
        assert_eq!(
            owner("package:npm/react", &mut diagnostics)
                .unwrap()
                .as_deref(),
            Some("libs")
        );
        assert_eq!(
            owner("package:python/numpy", &mut diagnostics)
                .unwrap()
                .as_deref(),
            Some("packages")
        );
        let error = owner("package:npm/yaml", &mut diagnostics)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("package `package:npm/yaml` matches unrelated"),
            "{error}"
        );
        let warn = yaml("{ambiguous_mapping: warn}");
        assert_eq!(
            package_owner("package:npm/yaml", &warn, &mut diagnostics).unwrap(),
            None
        );
        assert_eq!(diagnostics.warnings.len(), 1);
    }
}
