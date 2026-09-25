use crate::{config::ValidatedConfig, paths::relative_file};
use anyhow::{bail, Context, Result};
use ignore::WalkBuilder;
use std::{collections::BTreeSet, path::Path};

fn under(path: &str, root: &str) -> bool {
    root.is_empty()
        || path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// A single repository-root walk preserves ancestor .gitignore semantics even
/// when a source root itself is ignored. Directory pruning avoids unrelated trees.
pub fn discover(root: &Path, validated: &ValidatedConfig) -> Result<Vec<String>> {
    let source_roots = validated.config.project.source_roots.clone();
    for relative in &source_roots {
        let path = root.join(relative);
        if !path.exists() {
            bail!("source root `{relative}` does not exist; update project.source_roots");
        }
        let canonical = path
            .canonicalize()
            .with_context(|| format!("cannot resolve source root `{relative}`"))?;
        if !canonical.starts_with(root) {
            bail!("source root `{relative}` resolves outside the repository; use a repository-local path");
        }
        if path.symlink_metadata()?.file_type().is_symlink() {
            bail!(
                "source root `{relative}` is a symlink; configure its real repository-local path"
            );
        }
    }
    let owned_root = root.to_path_buf();
    let exclusions = validated.exclude_globs.clone();
    let roots_for_walk = source_roots.clone();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(false)
        .require_git(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .parents(true)
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            // Never ingest Git internals or our own/provider-generated output.
            let name = entry.file_name().to_string_lossy();
            if name == ".git" || name == ".archgraph" || name == ".gitnexus" {
                return false;
            }
            let Ok(relative) = relative_file(&owned_root, entry.path()) else {
                // Keep it so the checked walk below emits an actionable error.
                return true;
            };
            let is_directory = entry.file_type().is_some_and(|t| t.is_dir());
            if exclusions.is_match(&relative)
                || (is_directory && exclusions.is_match(format!("{relative}/")))
            {
                return false;
            }
            roots_for_walk.iter().any(|source| {
                under(&relative, source) || (is_directory && under(source, &relative))
            })
        });
    let mut files = BTreeSet::new();
    for result in builder.build() {
        let entry = result
            .context("failed to discover repository files; check permissions and ignore files")?;
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let path = relative_file(root, entry.path())?;
        if source_roots.iter().any(|source| under(&path, source))
            && !validated.exclude_globs.is_match(&path)
        {
            files.insert(path);
        }
    }
    Ok(files.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn respects_gitignore_roots_exclusions_and_git_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for directory in ["src/cache", "src/keep", ".git", "other", ".archgraph"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(root.join(".gitignore"), "src/ignored.rs\n").unwrap();
        for file in [
            "src/good.rs",
            "src/ignored.rs",
            "src/cache/x.rs",
            "src/keep/a.rs",
            "other/b.rs",
            ".git/config",
            ".archgraph/x",
        ] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let c = crate::config::parse("version: 1\nproject: {name: t, root: app, source_roots: [src], exclude: ['**/cache/**']}\nprovider: {kind: gitnexus}\nnodes: {app: {}}\n").unwrap();
        assert_eq!(
            discover(&root, &c).unwrap(),
            ["src/good.rs", "src/keep/a.rs"]
        );
    }
    #[test]
    fn ignored_source_root_does_not_bypass_gitignore() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("src")).unwrap();
        std::fs::write(t.path().join(".gitignore"), "src/\n").unwrap();
        std::fs::write(t.path().join("src/a.rs"), "").unwrap();
        let c = crate::config::parse("version: 1\nproject: {name: t, root: app, source_roots: [src]}\nprovider: {kind: gitnexus}\nnodes: {app: {}}\n").unwrap();
        assert!(discover(&t.path().canonicalize().unwrap(), &c)
            .unwrap()
            .is_empty());
    }
}
