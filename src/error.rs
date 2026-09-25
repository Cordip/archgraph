use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("failed to execute GitNexus `{command}`; install it or set GITNEXUS_BIN/provider.command: {source}")]
    Execute {
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("GitNexus {operation} failed ({status}): {detail}. Check provider.repo and run `gitnexus analyze --index-only`")]
    Command {
        operation: String,
        status: String,
        detail: String,
    },
    #[error("unsupported GitNexus cypher output; expected JSON object with `markdown` and `row_count` and a valid Markdown table: {0}. Check the GitNexus CLI version and adapter compatibility")]
    Compatibility(String),
    #[error("GitNexus {0} timed out; check the executable/index and rerun `gitnexus analyze --index-only`")]
    Timeout(String),
}

pub fn compatibility(detail: impl Into<String>) -> anyhow::Error {
    ProviderError::Compatibility(detail.into()).into()
}
