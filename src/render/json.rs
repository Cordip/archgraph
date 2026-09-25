use anyhow::{Context, Result};
use serde::Serialize;

pub fn render<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string_pretty(value).context("failed to serialize JSON output")
}
