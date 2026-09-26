//! Saved compiled architectures, and what changed since one. A refactoring
//! moves files and cuts dependencies over many steps; comparing the live
//! graph with a snapshot taken before it shows what the work has changed so
//! far: files added, removed or moved, dependencies gained and lost, and
//! violation observations that appeared or went away.
//!
//! Snapshots live in `.archgraph/snapshots/`, which ignores itself, so taking
//! one never changes the analyzed repository's Git status.
use crate::{
    baseline::{self, BaselineEntry},
    compiler::{work_directory, write_atomically},
    config::{is_within, overlaps},
    model::{ArchitectureIr, EdgeOrigin, EVIDENCE_LIMIT},
    projection::{self, EntryKind, Projection},
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_NAME: &str = "before";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub name: String,
    /// Git commit the snapshot was taken at; lets a diff follow files
    /// renamed since then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub ir: ArchitectureIr,
}

/// A saved snapshot as listed for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotInfo {
    pub name: String,
    pub commit: Option<String>,
}

/// Names become file names: letters, digits, `-`, `_` and `.`, not starting
/// with a dot, so a name can never leave the snapshot directory.
pub fn validate_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if !valid {
        bail!("invalid snapshot name `{name}`; use up to 64 letters, digits, `-`, `_` or `.`, not starting with `.`");
    }
    Ok(())
}

fn directory(root: &Path) -> PathBuf {
    root.join(".archgraph").join("snapshots")
}

pub fn path(root: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(directory(root).join(format!("{name}.json")))
}

pub fn save(
    root: &Path,
    name: &str,
    ir: &ArchitectureIr,
    commit: Option<String>,
) -> Result<PathBuf> {
    let destination = path(root, name)?;
    let snapshots = work_directory(root)?.join("snapshots");
    std::fs::create_dir_all(&snapshots)
        .with_context(|| format!("cannot create {}", snapshots.display()))?;
    let snapshot = Snapshot {
        schema_version: SCHEMA_VERSION,
        name: name.into(),
        commit,
        ir: ir.clone(),
    };
    let mut bytes = serde_json::to_vec_pretty(&snapshot).context("cannot serialize snapshot")?;
    bytes.push(b'\n');
    write_atomically(&destination, &bytes)?;
    Ok(destination)
}

pub fn load(root: &Path, name: &str) -> Result<Snapshot> {
    let source = path(root, name)?;
    let text = match std::fs::read_to_string(&source) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let names: Vec<String> = list(root)?.into_iter().map(|info| info.name).collect();
            let known = if names.is_empty() {
                "there are none; take one with `archgraph snapshot`".to_owned()
            } else {
                format!("saved: {}", names.join(", "))
            };
            bail!("no snapshot named `{name}` ({known})");
        }
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", source.display()))
        }
    };
    let snapshot: Snapshot = serde_json::from_str(&text).with_context(|| {
        format!(
            "{} is not a snapshot this version of ArchGraph can read; take it again with `archgraph snapshot --name {name}`",
            source.display()
        )
    })?;
    if snapshot.schema_version != SCHEMA_VERSION {
        bail!(
            "{}: unsupported snapshot schema {}; take it again with `archgraph snapshot --name {name}`",
            source.display(),
            snapshot.schema_version
        );
    }
    Ok(snapshot)
}

/// Saved snapshots by name. Reads only each file's name and commit.
pub fn list(root: &Path) -> Result<Vec<SnapshotInfo>> {
    #[derive(Deserialize)]
    struct Header {
        name: String,
        #[serde(default)]
        commit: Option<String>,
    }
    let entries = match std::fs::read_dir(directory(root)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("cannot list snapshots"),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.context("cannot list snapshots")?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = file_name.strip_suffix(".json") else {
            continue;
        };
        if validate_name(name).is_err() {
            continue;
        }
        // A snapshot file that cannot be read is still listed: loading it
        // then says what is wrong with it.
        let commit = std::fs::File::open(entry.path())
            .ok()
            .and_then(|file| {
                serde_json::from_reader::<_, Header>(std::io::BufReader::new(file)).ok()
            })
            .filter(|header| header.name == name)
            .and_then(|header| header.commit);
        found.push(SnapshotInfo {
            name: name.to_owned(),
            commit,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

// ------------------------------------------------------------------ diff

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct FileChange {
    pub path: String,
    pub node: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct FileMove {
    pub from: String,
    pub to: String,
    pub node_before: Option<String>,
    pub node_after: Option<String>,
}

/// A file whose owning node changed without the file moving: an edit of the
/// architecture's `maps`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Reassignment {
    pub path: String,
    pub node_before: Option<String>,
    pub node_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct DependencyChange {
    pub from_file: String,
    pub to_file: String,
    pub kind: String,
    pub from_node: String,
    pub to_node: String,
}

/// What changed in the whole architecture, or in one subtree of it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Diff {
    pub snapshot: String,
    pub commit: Option<String>,
    pub scope: Option<String>,
    pub files_added: Vec<FileChange>,
    pub files_removed: Vec<FileChange>,
    pub files_moved: Vec<FileMove>,
    pub files_reassigned: Vec<Reassignment>,
    pub dependencies_added: Vec<DependencyChange>,
    pub dependencies_removed: Vec<DependencyChange>,
    /// Observations behind violations: a rule and a file pair, as in a
    /// baseline.
    pub violations_appeared: Vec<BaselineEntry>,
    pub violations_resolved: Vec<BaselineEntry>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.files_added.is_empty()
            && self.files_removed.is_empty()
            && self.files_moved.is_empty()
            && self.files_reassigned.is_empty()
            && self.dependencies_added.is_empty()
            && self.dependencies_removed.is_empty()
            && self.violations_appeared.is_empty()
            && self.violations_resolved.is_empty()
    }
}

/// Old path -> new path for the snapshot's files that are gone now, taken
/// from Git's renames (`vcs::renames_since`). A rename of a file that still
/// exists under its old name, or of a file the snapshot did not have, is
/// ignored.
fn followed_renames(
    before: &ArchitectureIr,
    after: &ArchitectureIr,
    renames: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let before_files: BTreeSet<&str> = before.files.iter().map(|file| file.path.as_str()).collect();
    let after_files: BTreeSet<&str> = after.files.iter().map(|file| file.path.as_str()).collect();

    renames
        .iter()
        .filter(|(old, new)| {
            before_files.contains(old.as_str())
                && !after_files.contains(old.as_str())
                && after_files.contains(new.as_str())
                && !before_files.contains(new.as_str())
        })
        .map(|(old, new)| (old.clone(), new.clone()))
        .collect()
}

fn owners(ir: &ArchitectureIr) -> BTreeMap<&str, Option<&str>> {
    ir.files
        .iter()
        .map(|file| (file.path.as_str(), file.node.as_deref()))
        .collect()
}

fn dependencies(
    ir: &ArchitectureIr,
    moved: &dyn Fn(&str) -> String,
) -> BTreeMap<(String, String, String), (String, String)> {
    ir.resolved_edges
        .iter()
        .map(|edge| {
            let key = (
                moved(&edge.evidence.from_file),
                moved(&edge.evidence.to_file),
                edge.kind.clone(),
            );
            (key, (edge.from.clone(), edge.to.clone()))
        })
        .collect()
}

/// Compares a snapshot with the live architecture. `scope` keeps changes
/// that touch a subtree; `renames` (old -> new path) lets a moved file stay
/// one file rather than a removed and an added one.
pub fn diff(
    snapshot: &Snapshot,
    after: &ArchitectureIr,
    scope: Option<&str>,
    renames: &BTreeMap<String, String>,
) -> Diff {
    let before = &snapshot.ir;
    let renames = followed_renames(before, after, renames);
    let moved = |path: &str| {
        renames
            .get(path)
            .cloned()
            .unwrap_or_else(|| path.to_owned())
    };
    let in_scope = |node: Option<&str>| match (scope, node) {
        (None, _) => true,
        (Some(scope), Some(node)) => is_within(node, scope),
        (Some(_), None) => false,
    };
    let mut result = Diff {
        snapshot: snapshot.name.clone(),
        commit: snapshot.commit.clone(),
        scope: scope.map(str::to_owned),
        ..Diff::default()
    };

    let before_owners = owners(before);
    let after_owners = owners(after);
    for (&path, &node) in &after_owners {
        let old = renames
            .iter()
            .find(|(_, new)| new.as_str() == path)
            .map(|(old, _)| old.as_str());
        let before_node = match old {
            Some(old) => before_owners.get(old).copied().flatten(),
            None => match before_owners.get(path) {
                Some(&before_node) => before_node,
                None => {
                    if in_scope(node) {
                        result.files_added.push(FileChange {
                            path: path.into(),
                            node: node.map(str::to_owned),
                        });
                    }
                    continue;
                }
            },
        };
        if !in_scope(node) && !in_scope(before_node) {
            continue;
        }
        if let Some(old) = old {
            result.files_moved.push(FileMove {
                from: old.into(),
                to: path.into(),
                node_before: before_node.map(str::to_owned),
                node_after: node.map(str::to_owned),
            });
        } else if before_node != node {
            result.files_reassigned.push(Reassignment {
                path: path.into(),
                node_before: before_node.map(str::to_owned),
                node_after: node.map(str::to_owned),
            });
        }
    }
    for (&path, &node) in &before_owners {
        if !after_owners.contains_key(path) && !renames.contains_key(path) && in_scope(node) {
            result.files_removed.push(FileChange {
                path: path.into(),
                node: node.map(str::to_owned),
            });
        }
    }

    // Dependencies are compared by file pair and kind, with the snapshot's
    // paths moved to where their files are now.
    let touches = |nodes: &(String, String)| {
        scope.is_none_or(|scope| overlaps(&nodes.0, scope) || overlaps(&nodes.1, scope))
    };
    let change = |(from_file, to_file, kind): &(String, String, String),
                  (from_node, to_node): &(String, String)| DependencyChange {
        from_file: from_file.clone(),
        to_file: to_file.clone(),
        kind: kind.clone(),
        from_node: from_node.clone(),
        to_node: to_node.clone(),
    };
    let before_dependencies = dependencies(before, &moved);
    let after_dependencies = dependencies(after, &|path: &str| path.to_owned());
    for (key, nodes) in &after_dependencies {
        if !before_dependencies.contains_key(key) && touches(nodes) {
            result.dependencies_added.push(change(key, nodes));
        }
    }
    for (key, nodes) in &before_dependencies {
        if !after_dependencies.contains_key(key) && touches(nodes) {
            result.dependencies_removed.push(change(key, nodes));
        }
    }

    // Violations are compared observation by observation, like a baseline.
    let entry_in_scope = |entry: &BaselineEntry, owners: &BTreeMap<&str, Option<&str>>| {
        scope.is_none()
            || in_scope(owners.get(entry.from_file.as_str()).copied().flatten())
            || in_scope(owners.get(entry.to_file.as_str()).copied().flatten())
    };
    let before_entries: BTreeSet<BaselineEntry> = baseline::current_entries(before)
        .into_iter()
        .map(|entry| BaselineEntry {
            from_file: moved(&entry.from_file),
            to_file: moved(&entry.to_file),
            ..entry
        })
        .collect();
    let after_entries = baseline::current_entries(after);
    let moved_before_owners: BTreeMap<String, Option<&str>> = before_owners
        .iter()
        .map(|(&path, &node)| (moved(path), node))
        .collect();
    let moved_before_owners: BTreeMap<&str, Option<&str>> = moved_before_owners
        .iter()
        .map(|(path, &node)| (path.as_str(), node))
        .collect();
    result.violations_appeared = after_entries
        .difference(&before_entries)
        .filter(|entry| entry_in_scope(entry, &after_owners))
        .cloned()
        .collect();
    result.violations_resolved = before_entries
        .difference(&after_entries)
        .filter(|entry| entry_in_scope(entry, &moved_before_owners))
        .cloned()
        .collect();

    result.files_added.sort();
    result.files_removed.sort();
    result.files_moved.sort();
    result.files_reassigned.sort();
    result
}

// ------------------------------------------------------------ level diff

/// An entry of the level that the snapshot had and the live graph has not.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RemovedEntry {
    pub id: String,
    pub title: String,
    pub entry_kind: EntryKind,
    pub file_path: Option<String>,
    pub architecture_id: Option<String>,
    pub outside_focus: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MovedEntry {
    /// The entry's ID in the snapshot and its ID now.
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResizedEntry {
    pub id: String,
    pub file_count_before: usize,
    pub file_count_after: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct EdgeKey {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub origin: EdgeOrigin,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EdgeCount {
    #[serde(flatten)]
    pub key: EdgeKey,
    pub count_before: usize,
    pub count_after: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ViolationKey {
    pub rule_id: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub edge_kind: Option<String>,
    /// A cycle's members; empty for other rules.
    pub nodes: Vec<String>,
}

/// What changed on one level of the UI, in the live projection's entry IDs.
/// Edges that are gone name their endpoints as they are now, so an edge
/// between two entries that still exist can be drawn between them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LevelDiff {
    pub snapshot: String,
    pub commit: Option<String>,
    pub focus: String,
    /// Whether the snapshot had this node; if not, everything on it is new.
    pub focus_existed: bool,
    pub entries_added: Vec<String>,
    pub entries_removed: Vec<RemovedEntry>,
    pub entries_moved: Vec<MovedEntry>,
    pub entries_resized: Vec<ResizedEntry>,
    pub edges_added: Vec<EdgeKey>,
    pub edges_removed: Vec<EdgeCount>,
    pub edges_changed: Vec<EdgeCount>,
    pub violations_appeared: Vec<ViolationKey>,
    pub violations_resolved: Vec<ViolationKey>,
    /// The subtree's file-level diff.
    pub summary: Diff,
}

fn edge_counts(view: &Projection, rename: &dyn Fn(&str) -> String) -> BTreeMap<EdgeKey, usize> {
    let mut counts = BTreeMap::new();
    for projected in &view.edges {
        let edge = &projected.edge;
        let key = EdgeKey {
            from: rename(&edge.from),
            to: rename(&edge.to),
            kind: edge.kind.clone(),
            origin: edge.origin,
        };
        *counts.entry(key).or_insert(0) += edge.count;
    }
    counts
}

fn violation_keys(view: &Projection) -> BTreeSet<ViolationKey> {
    view.violations
        .iter()
        .map(|violation| ViolationKey {
            rule_id: violation.rule_id.clone(),
            from: violation.from.clone(),
            to: violation.to.clone(),
            edge_kind: violation.edge_kind.clone(),
            nodes: if violation.from.is_none() {
                violation.nodes.clone()
            } else {
                Vec::new()
            },
        })
        .collect()
}

/// Compares one level of the snapshot with the same level now.
pub fn level_diff(
    snapshot: &Snapshot,
    after: &ArchitectureIr,
    focus: &str,
    renames: &BTreeMap<String, String>,
) -> Result<LevelDiff> {
    let now = projection::project(after, focus, EVIDENCE_LIMIT)?;
    let then = if snapshot.ir.nodes.contains_key(focus) {
        Some(projection::project(&snapshot.ir, focus, EVIDENCE_LIMIT)?)
    } else {
        None
    };
    let followed = followed_renames(&snapshot.ir, after, renames);
    // Entry IDs of moved files follow their files; `file:` is the only ID
    // that carries a path.
    let rename = |id: &str| match id.strip_prefix("file:").and_then(|path| followed.get(path)) {
        Some(new) => format!("file:{new}"),
        None => id.to_owned(),
    };
    let mut result = LevelDiff {
        snapshot: snapshot.name.clone(),
        commit: snapshot.commit.clone(),
        focus: focus.into(),
        focus_existed: then.is_some(),
        entries_added: Vec::new(),
        entries_removed: Vec::new(),
        entries_moved: Vec::new(),
        entries_resized: Vec::new(),
        edges_added: Vec::new(),
        edges_removed: Vec::new(),
        edges_changed: Vec::new(),
        violations_appeared: Vec::new(),
        violations_resolved: Vec::new(),
        summary: diff(snapshot, after, Some(focus), renames),
    };
    let Some(then) = then else {
        result.entries_added = now.nodes.iter().map(|node| node.id.clone()).collect();
        result.edges_added = edge_counts(&now, &|id: &str| id.to_owned())
            .into_keys()
            .collect();
        result.violations_appeared = violation_keys(&now).into_iter().collect();
        return Ok(result);
    };

    let now_entries: BTreeMap<&str, &projection::ProjectionNode> = now
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut matched = BTreeSet::new();
    for node in &then.nodes {
        let id = rename(&node.id);
        match now_entries.get(id.as_str()) {
            Some(current) => {
                matched.insert(id.clone());
                if id != node.id {
                    result.entries_moved.push(MovedEntry {
                        from: node.id.clone(),
                        to: id,
                    });
                } else if current.file_count != node.file_count {
                    result.entries_resized.push(ResizedEntry {
                        id,
                        file_count_before: node.file_count,
                        file_count_after: current.file_count,
                    });
                }
            }
            None => result.entries_removed.push(RemovedEntry {
                id: node.id.clone(),
                title: node.title.clone(),
                entry_kind: node.entry_kind,
                file_path: node.file_path.clone(),
                architecture_id: node.architecture_id.clone(),
                outside_focus: node.outside_focus,
            }),
        }
    }
    result.entries_added = now
        .nodes
        .iter()
        .filter(|node| !matched.contains(&node.id))
        .map(|node| node.id.clone())
        .collect();

    let before_edges = edge_counts(&then, &rename);
    let after_edges = edge_counts(&now, &|id: &str| id.to_owned());
    for (key, &count_after) in &after_edges {
        match before_edges.get(key) {
            None => result.edges_added.push(key.clone()),
            Some(&count_before) if count_before != count_after => {
                result.edges_changed.push(EdgeCount {
                    key: key.clone(),
                    count_before,
                    count_after,
                })
            }
            Some(_) => {}
        }
    }
    for (key, &count_before) in &before_edges {
        if !after_edges.contains_key(key) {
            result.edges_removed.push(EdgeCount {
                key: key.clone(),
                count_before,
                count_after: 0,
            });
        }
    }

    let before_violations = violation_keys(&then);
    let after_violations = violation_keys(&now);
    result.violations_appeared = after_violations
        .difference(&before_violations)
        .cloned()
        .collect();
    result.violations_resolved = before_violations
        .difference(&after_violations)
        .cloned()
        .collect();
    Ok(result)
}
