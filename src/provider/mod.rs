//! External indexing/querying is isolated here; core graph code is provider-neutral.
pub mod css;
pub mod gitnexus;
pub mod markdown_table;

use crate::model::{CodeEdge, ProviderInfo};
use anyhow::{bail, Result};
use async_trait::async_trait;

/// How to refresh the provider's index before compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ReindexMode {
    /// Reuse cached analysis for unchanged files (fast; the default).
    Incremental,
    /// Rebuild from scratch. Needed after changing resolver configuration
    /// (package.json, tsconfig, workspace files), which the cache ignores.
    Full,
}

#[async_trait]
pub trait CodeGraphProvider: Send + Sync {
    /// Verify availability, indexing, and the expected query schema.
    async fn info(&self) -> Result<ProviderInfo>;
    async fn dependency_edges(&self) -> Result<Vec<CodeEdge>>;
    /// Every file path in the provider's index, if the provider can list them.
    /// Lets the compiler tell files it never indexed (unsupported language,
    /// ignored directory) from indexed files without dependencies.
    async fn indexed_files(&self) -> Result<Option<Vec<String>>> {
        Ok(None)
    }
    /// Cheap identity of the provider's current index, if it has one. The
    /// compiler compares it before and after querying to detect an index
    /// rewritten mid-compile (e.g. by an auto-index service).
    async fn fingerprint(&self) -> Result<Option<String>> {
        Ok(None)
    }
    /// What `info`, `dependency_edges` and `indexed_files` depend on besides
    /// the index itself: queries, their parameters, the executable. With
    /// `fingerprint` it keys the result cache; `None` disables caching.
    fn query_identity(&self) -> Option<String> {
        None
    }
    async fn reindex(&self, _mode: ReindexMode) -> Result<()> {
        bail!("this code-graph provider does not support reindexing; index it externally before compiling")
    }
}

/// A real in-memory provider, available to library consumers and integration tests.
/// The CLI never substitutes this for an unavailable external provider.
#[derive(Debug, Clone, Default)]
pub struct InMemoryProvider {
    pub edges: Vec<CodeEdge>,
    pub failure: Option<String>,
    /// Successive `fingerprint()` results; the last one repeats. Empty: none.
    pub fingerprints: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// Files reported as indexed; `None` means the provider cannot list them.
    pub indexed: Option<Vec<String>>,
    /// Number of `dependency_edges` calls, shared between clones.
    pub queries: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
#[async_trait]
impl CodeGraphProvider for InMemoryProvider {
    async fn info(&self) -> Result<ProviderInfo> {
        if let Some(message) = &self.failure {
            bail!("{message}");
        }
        Ok(ProviderInfo {
            provider: "in-memory".into(),
            version: None,
            repository: None,
        })
    }
    async fn dependency_edges(&self) -> Result<Vec<CodeEdge>> {
        if let Some(message) = &self.failure {
            bail!("{message}");
        }
        self.queries
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(self.edges.clone())
    }
    async fn indexed_files(&self) -> Result<Option<Vec<String>>> {
        Ok(self.indexed.clone())
    }
    async fn fingerprint(&self) -> Result<Option<String>> {
        let mut sequence = self.fingerprints.lock().expect("fingerprint lock");
        Ok(match sequence.len() {
            0 => None,
            1 => sequence.first().cloned(),
            _ => Some(sequence.remove(0)),
        })
    }
    fn query_identity(&self) -> Option<String> {
        Some("in-memory".into())
    }
    async fn reindex(&self, _mode: ReindexMode) -> Result<()> {
        if let Some(message) = &self.failure {
            bail!("{message}");
        }
        Ok(())
    }
}
