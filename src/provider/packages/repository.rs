//! What the repository itself provides, to tell a local import from a
//! package: its Python modules and packages, the names of its npm packages
//! (workspaces) and the dependencies each `package.json` declares.
use crate::paths::relative_file;
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
};

/// Directories that hold installed packages, not repository code.
const INSTALLED: [&str; 3] = ["node_modules", "site-packages", "__pycache__"];

pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

fn join(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}

#[derive(Debug, Default)]
pub struct Repository {
    /// Directory (`""` is the root) -> Python modules and packages directly
    /// in it: `x.py`, or a subdirectory holding Python files (with or
    /// without `__init__.py`).
    python: HashMap<String, BTreeSet<String>>,
    /// Python module or package name -> the first directory holding it, in
    /// path order.
    python_anywhere: BTreeMap<String, String>,
    /// The `name` of every `package.json` in the repository.
    workspace: BTreeSet<String>,
    /// Directory -> dependency names its `package.json` declares.
    manifests: BTreeMap<String, BTreeSet<String>>,
    pub warnings: Vec<String>,
}

impl Repository {
    /// Walks the whole repository, ignoring what Git ignores but not the
    /// configuration's `exclude` or `source_roots`: an excluded module is
    /// still local.
    pub fn read(root: &Path) -> Result<Self> {
        let mut repository = Self::default();
        let mut builder = WalkBuilder::new(root);
        builder
            .hidden(false)
            .follow_links(false)
            .require_git(false)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(false)
            .parents(true)
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                entry.depth() == 0
                    || !(name == ".git"
                        || name == ".archgraph"
                        || name == ".gitnexus"
                        || INSTALLED.contains(&name.as_ref()))
            });
        let mut manifests = Vec::new();
        for entry in builder.build() {
            let entry = entry.context("cannot walk the repository for local modules")?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let Ok(path) = relative_file(root, entry.path()) else {
                continue;
            };
            if path.ends_with(".py") {
                repository.add_python(&path);
            } else if path == "package.json" || path.ends_with("/package.json") {
                manifests.push(path);
            }
        }
        manifests.sort();
        for path in manifests {
            repository.add_manifest(root, &path);
        }
        Ok(repository)
    }

    fn add_python(&mut self, path: &str) {
        let stem = path.strip_suffix(".py").unwrap_or(path);
        let mut directory = parent(stem);
        let name = stem.rsplit('/').next().unwrap_or(stem);
        if name != "__init__" {
            self.add_python_name(directory, name);
        }
        // Every directory above the file is a (namespace) package.
        while !directory.is_empty() {
            let above = parent(directory);
            let name = directory.rsplit('/').next().unwrap_or(directory).to_owned();
            self.add_python_name(above, &name);
            directory = above;
        }
    }

    fn add_python_name(&mut self, directory: &str, name: &str) {
        self.python
            .entry(directory.to_owned())
            .or_default()
            .insert(name.to_owned());
        let first = self
            .python_anywhere
            .entry(name.to_owned())
            .or_insert_with(|| directory.to_owned());
        // The walk order is the file system's; keep the smallest for stable output.
        if directory < first.as_str() {
            *first = directory.to_owned();
        }
    }

    fn add_manifest(&mut self, root: &Path, path: &str) {
        let parsed = std::fs::read(root.join(path))
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(serde_json::from_slice::<serde_json::Value>(&bytes)?));
        let manifest = match parsed {
            Ok(manifest) => manifest,
            Err(error) => {
                self.warnings.push(format!("cannot read {path} ({error}); imports below it are not known to be packages and are reported as ambiguous"));
                return;
            }
        };
        if let Some(name) = manifest.get("name").and_then(|name| name.as_str()) {
            self.workspace.insert(name.to_owned());
        }
        let declared = self.manifests.entry(parent(path).to_owned()).or_default();
        for field in [
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ] {
            if let Some(dependencies) = manifest.get(field).and_then(|value| value.as_object()) {
                declared.extend(dependencies.keys().cloned());
            }
        }
    }

    /// Whether the Python module `name` sits beside `file`, in a directory
    /// above it or in a `src/` directory above it: the import roots a test
    /// runner or a script started from those directories uses.
    pub fn python_reachable(&self, file: &str, name: &str) -> bool {
        let mut directory = parent(file);
        loop {
            for candidate in [directory.to_owned(), join(directory, "src")] {
                if self
                    .python
                    .get(&candidate)
                    .is_some_and(|names| names.contains(name))
                {
                    return true;
                }
            }
            if directory.is_empty() {
                return false;
            }
            directory = parent(directory);
        }
    }

    /// A directory elsewhere in the repository holding a Python module or
    /// package `name`.
    pub fn python_elsewhere(&self, name: &str) -> Option<&str> {
        self.python_anywhere.get(name).map(String::as_str)
    }

    pub fn workspace_package(&self, name: &str) -> bool {
        self.workspace.contains(name)
    }

    /// Whether a `package.json` beside `file` or above it declares `name`, or
    /// its type declarations `@types/name`.
    pub fn declared(&self, file: &str, name: &str) -> bool {
        let types = match name.strip_prefix('@') {
            Some(scoped) => format!("@types/{}", scoped.replacen('/', "__", 1)),
            None => format!("@types/{name}"),
        };
        let mut directory = parent(file);
        loop {
            if self
                .manifests
                .get(directory)
                .is_some_and(|declared| declared.contains(name) || declared.contains(&types))
            {
                return true;
            }
            if directory.is_empty() {
                return false;
            }
            directory = parent(directory);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_python_modules_packages_and_manifests() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for (path, content) in [
            ("backend/planner/__init__.py", ""),
            ("backend/planner/core/solver.py", ""),
            ("backend/tests/test_solver.py", ""),
            ("lib/src/mylib/__init__.py", ""),
            ("lib/tests/test_lib.py", ""),
            ("tools/redis.py", ""),
            ("web/node_modules/react/index.py", ""),
            (
                "web/package.json",
                r#"{"name": "web", "dependencies": {"react": "19"}, "devDependencies": {"@types/geojson": "7"}}"#,
            ),
            ("web/packages/ui/package.json", r#"{"name": "@acme/ui"}"#),
            ("broken/package.json", "{"),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let repository = Repository::read(root).unwrap();
        assert!(repository.python_reachable("backend/tests/test_solver.py", "planner"));
        assert!(repository.python_reachable("backend/tests/test_solver.py", "tests"));
        assert!(repository.python_reachable("backend/planner/core/solver.py", "solver"));
        assert!(repository.python_reachable("lib/tests/test_lib.py", "mylib"));
        assert!(!repository.python_reachable("backend/tests/test_solver.py", "redis"));
        assert_eq!(repository.python_elsewhere("redis"), Some("tools"));
        assert_eq!(repository.python_elsewhere("react"), None);
        assert!(repository.declared("web/src/app.tsx", "react"));
        assert!(repository.declared("web/src/app.tsx", "geojson"));
        assert!(!repository.declared("backend/app.ts", "react"));
        assert!(repository.workspace_package("@acme/ui"));
        assert!(repository.warnings[0].contains("cannot read broken/package.json"));
    }
}
