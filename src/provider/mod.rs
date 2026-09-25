//! External indexing/querying is isolated here; core graph code is provider-neutral.
pub mod gitnexus;
pub mod markdown_table;

use crate::model::{CodeEdge, ProviderInfo};
use anyhow::{bail, Result};
use async_trait::async_trait;

#[async_trait]
pub trait CodeGraphProvider: Send + Sync {
    /// Verify availability, indexing, and the expected query schema.
    async fn info(&self) -> Result<ProviderInfo>;
    async fn dependency_edges(&self) -> Result<Vec<CodeEdge>>;
    async fn reindex(&self) -> Result<()> {
        bail!("this code-graph provider does not support reindexing; index it externally before compiling")
    }
}

/// A real in-memory provider, available to library consumers and integration tests.
/// The CLI never substitutes this for an unavailable external provider.
#[derive(Debug, Clone, Default)]
pub struct InMemoryProvider {
    pub edges: Vec<CodeEdge>,
    pub failure: Option<String>,
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
        Ok(self.edges.clone())
    }
    async fn reindex(&self) -> Result<()> {
        if let Some(message) = &self.failure {
            bail!("{message}");
        }
        Ok(())
    }
}
