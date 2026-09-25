use crate::{config::{is_within, AmbiguityPolicy, FilePolicy, ValidatedConfig}, model::{CompiledFile, Diagnostics}, paths::normalize_relative};
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
    let paths: BTreeSet<String> = files.iter().map(|path| normalize_relative(path)).collect::<Result<_>>()?;
    for path in paths {
        let matches: BTreeSet<String> = validated.mapping_globs.matches(&path).into_iter()
            .map(|index| validated.mapping_owners[index].clone()).collect();
        let mut ranked: Vec<String> = matches.into_iter().collect();
        ranked.sort_by(|a, b| a.split('.').count().cmp(&b.split('.').count()).then_with(|| a.cmp(b)));
        let mut file = CompiledFile { path: path.clone(), node: None, ambiguous_matches: Vec::new() };
        if ranked.is_empty() {
            diagnostics.unassigned_files.push(path.clone());
            match validated.config.policies.unassigned_files {
                FilePolicy::Error => bail!("file `{path}` is unassigned; add a node maps glob or change policies.unassigned_files"),
                FilePolicy::Warn => {}
                FilePolicy::Ignore => {}
            }
        } else if ranked.windows(2).all(|pair| is_within(&pair[1], &pair[0])) {
            file.node = ranked.last().cloned();
        } else {
            let unrelated = ranked.iter().enumerate().find_map(|(i, a)| ranked.iter().skip(i + 1)
                .find(|b| !is_within(a, b) && !is_within(b, a)).map(|b| (a, b)));
            if let Some((a, b)) = unrelated {
                let message = format!("file `{path}` matches unrelated architecture nodes `{a}` and `{b}`; make maps unambiguous or change policies.ambiguous_mapping");
                if validated.config.policies.ambiguous_mapping == AmbiguityPolicy::Error { bail!("{message}"); }
                diagnostics.warnings.push(message);
            }
            ranked.sort();
            file.ambiguous_matches = ranked.clone();
            diagnostics.ambiguous_files.insert(path, ranked);
            // Warn is never permission to arbitrarily assign an unrelated branch.
        }
        memberships.push(file);
    }
    if !diagnostics.unassigned_files.is_empty() && validated.config.policies.unassigned_files == FilePolicy::Warn {
        diagnostics.warnings.push(format!("{} unassigned file(s); add maps globs or adjust policies.unassigned_files", diagnostics.unassigned_files.len()));
    }
    diagnostics.warnings.sort();
    Ok(Memberships { files: memberships, diagnostics })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(nodes: &str, policies: &str) -> ValidatedConfig {
        crate::config::parse(&format!("version: 1\nproject: {{name: t, root: app}}\nprovider: {{kind: gitnexus}}\npolicies: {policies}\nnodes:\n  app: {{maps: ['src/**']}}\n{nodes}")).unwrap()
    }
    #[test]
    fn deepest_match_and_cross_platform_paths() {
        let c = config("  app.foo: {maps: ['src/foo/**']}\n  app.foo.api: {maps: ['src/foo/api/**']}\n", "{}");
        let m = resolve(&[r"src\foo\api\a.rs".into(), "src/b.rs".into()], &c).unwrap();
        assert_eq!(m.files[0].node.as_deref(), Some("app"));
        assert_eq!(m.files[1].node.as_deref(), Some("app.foo.api"));
    }
    #[test]
    fn unrelated_branches_error_by_default() {
        let c = config("  app.foo: {maps: ['src/**']}\n  app.foobar: {maps: ['src/**']}\n", "{}");
        assert!(resolve(&["src/a.rs".into()], &c).unwrap_err().to_string().contains("unrelated architecture nodes"));
    }
    #[test]
    fn warning_keeps_ambiguity_explicit() {
        let c = config("  app.a: {maps: ['src/**']}\n  app.b: {maps: ['src/**']}\n", "{ambiguous_mapping: warn}");
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
            if policy == "error" { assert!(result.is_err()); } else {
                let result = result.unwrap();
                assert_eq!(result.diagnostics.unassigned_files, ["other/a"]);
                assert_eq!(result.diagnostics.warnings.is_empty(), policy == "ignore");
            }
        }
    }
}
