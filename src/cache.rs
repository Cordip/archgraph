//! Provider results kept between runs. `show` and `context` recompile on every
//! call, and against an unchanged index each run repeated the same GitNexus
//! queries (seconds on a large repository). Only raw provider output is cached,
//! keyed on the provider's index fingerprint and query identity; configuration,
//! file discovery and everything derived from them are recomputed every run.
use crate::model::{CodeEdge, ProviderInfo, Route};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

const SCHEMA_VERSION: u32 = 2;

/// Everything the compiler reads from the provider for one index state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub info: ProviderInfo,
    pub edges: Vec<CodeEdge>,
    pub indexed_files: Option<Vec<String>>,
    pub routes: Option<Vec<Route>>,
}

#[derive(Serialize, Deserialize)]
struct Entry<S> {
    schema_version: u32,
    archgraph_version: String,
    key: String,
    snapshot: S,
}

/// `None` when the provider cannot identify its index or its queries: such
/// results are never cached.
pub fn key(fingerprint: Option<&str>, query_identity: Option<&str>) -> Option<String> {
    Some(format!("{}\n{}", fingerprint?, query_identity?))
}

/// A missing, unreadable, outdated or mismatched cache is a miss, never an error.
pub fn load(path: &Path, key: &str) -> Option<Snapshot> {
    let text = std::fs::read(path).ok()?;
    let entry: Entry<Snapshot> = serde_json::from_slice(&text).ok()?;
    (entry.schema_version == SCHEMA_VERSION
        && entry.archgraph_version == env!("CARGO_PKG_VERSION")
        && entry.key == key)
        .then_some(entry.snapshot)
}

pub fn store(path: &Path, key: &str, snapshot: &Snapshot) -> Result<()> {
    let bytes = serde_json::to_vec(&Entry {
        schema_version: SCHEMA_VERSION,
        archgraph_version: env!("CARGO_PKG_VERSION").into(),
        key: key.into(),
        snapshot,
    })
    .context("cannot serialize the provider cache")?;
    let directory = path
        .parent()
        .context("cache path has no parent directory")?;
    std::fs::create_dir_all(directory)
        .with_context(|| format!("cannot create {}", directory.display()))?;
    crate::compiler::write_atomically(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot {
        Snapshot {
            info: ProviderInfo {
                provider: "test".into(),
                version: Some("1".into()),
                repository: None,
            },
            edges: vec![CodeEdge {
                from_file: "a".into(),
                to_file: "b".into(),
                kind: "IMPORTS".into(),
                confidence: Some(0.9),
                reason: None,
            }],
            indexed_files: Some(vec!["a".into(), "b".into()]),
            routes: None,
        }
    }

    #[test]
    fn round_trips_only_under_the_same_key() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cache/provider.json");
        let key = key(Some("index-1"), Some("queries")).unwrap();
        assert_eq!(load(&path, &key), None);
        store(&path, &key, &snapshot()).unwrap();
        assert_eq!(load(&path, &key), Some(snapshot()));
        let changed = super::key(Some("index-2"), Some("queries")).unwrap();
        assert_eq!(load(&path, &changed), None);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path, &key), None);
    }

    #[test]
    fn unidentified_results_have_no_key() {
        assert_eq!(key(None, Some("queries")), None);
        assert_eq!(key(Some("index"), None), None);
    }
}
