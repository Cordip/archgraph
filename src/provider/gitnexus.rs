use super::{
    markdown_table::{self, Table},
    CodeGraphProvider,
};
use crate::{
    config::ProviderConfig,
    error::{compatibility, ProviderError},
    model::{CodeEdge, ProviderInfo},
};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::{
    ffi::OsString,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{process::Command, time::timeout};

const PROBE: &str = "MATCH (f:File) RETURN f.filePath AS path LIMIT 1";

#[derive(Debug)]
pub struct GitNexusCliProvider {
    executable: OsString,
    root: PathBuf,
    repository: Option<String>,
    page_size: usize,
    edge_types: Vec<String>,
}

#[derive(Debug)]
pub struct CypherPage {
    pub table: Table,
    pub row_count: usize,
}

pub fn parse_wrapper(stdout: &str) -> Result<CypherPage> {
    let json: Value = serde_json::from_str(stdout)
        .map_err(|error| compatibility(format!("stdout is not JSON: {error}")))?;
    let object = json
        .as_object()
        .ok_or_else(|| compatibility("stdout is not a JSON object"))?;
    let markdown = object
        .get("markdown")
        .and_then(Value::as_str)
        .ok_or_else(|| compatibility("missing or non-string `markdown`"))?;
    let count = object
        .get("row_count")
        .and_then(Value::as_u64)
        .ok_or_else(|| compatibility("missing or nonnegative-integer `row_count`"))?;
    let row_count = usize::try_from(count)
        .map_err(|_| compatibility("row_count exceeds this platform's capacity"))?;
    let table = markdown_table::parse(markdown)?;
    if table.rows.len() != row_count {
        return Err(compatibility(format!(
            "row_count is {row_count}, but table contains {} data rows (truncation is unsupported)",
            table.rows.len()
        )));
    }
    Ok(CypherPage { table, row_count })
}

fn present(value: &str) -> bool {
    !value.is_empty() && !value.eq_ignore_ascii_case("null")
}

pub fn parse_edge_page(stdout: &str) -> Result<Vec<CodeEdge>> {
    let page = parse_wrapper(stdout)?;
    let source = page.table.column("source")?;
    let target = page.table.column("target")?;
    let kind = page.table.optional_column("kind");
    let confidence = page.table.optional_column("confidence");
    let reason = page.table.optional_column("reason");
    let mut edges = Vec::with_capacity(page.row_count);
    for (index, row) in page.table.rows.iter().enumerate() {
        if !present(&row[source]) || !present(&row[target]) {
            return Err(compatibility(format!(
                "row {} has empty/null source or target",
                index + 1
            )));
        }
        let confidence = confidence
            .map(|column| row[column].as_str())
            .filter(|value| present(value))
            .map(|value| {
                let number = value.parse::<f64>().map_err(|_| {
                    compatibility(format!(
                        "row {} has invalid confidence `{value}`",
                        index + 1
                    ))
                })?;
                if !number.is_finite() {
                    return Err(compatibility(format!(
                        "row {} has non-finite confidence",
                        index + 1
                    )));
                }
                Ok(number)
            })
            .transpose()?;
        edges.push(CodeEdge {
            // Lexical absolute/root validation happens in the compiler, where
            // anomalies can be reported against the discovered file universe.
            from_file: row[source].replace('\\', "/"),
            to_file: row[target].replace('\\', "/"),
            kind: match kind {
                Some(column) if present(&row[column]) => row[column].clone(),
                Some(_) => {
                    return Err(compatibility(format!(
                        "row {} has an empty/null relation kind",
                        index + 1
                    )))
                }
                None => "IMPORTS".into(),
            },
            confidence,
            reason: reason
                .map(|column| row[column].clone())
                .filter(|value| present(value)),
        });
    }
    Ok(edges)
}

/// Every file in the index, to tell unindexed files from files without
/// dependencies.
pub fn file_query(offset: usize, page_size: usize) -> String {
    format!(
        "MATCH (f:File) RETURN f.filePath AS path ORDER BY path SKIP {offset} LIMIT {page_size}"
    )
}

pub fn parse_file_page(stdout: &str) -> Result<Vec<String>> {
    let page = parse_wrapper(stdout)?;
    let path = page.table.column("path")?;
    page.table
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            if present(&row[path]) {
                Ok(row[path].replace('\\', "/"))
            } else {
                Err(compatibility(format!(
                    "file row {} has an empty/null path",
                    index + 1
                )))
            }
        })
        .collect()
}

/// File-level relations of the given types. Symbol endpoints are lifted to
/// their files and grouped, so every row is unique and SKIP/LIMIT paging is
/// stable. Edge types are validated identifiers (`config::valid_edge_type`).
pub fn edge_query(edge_types: &[String], offset: usize, page_size: usize) -> String {
    let types = edge_types
        .iter()
        .map(|kind| format!("'{kind}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "MATCH (a)-[r:CodeRelation]->(b) WHERE r.type IN [{types}] \
AND a.filePath IS NOT NULL AND b.filePath IS NOT NULL AND a.filePath <> b.filePath \
RETURN a.filePath AS source, b.filePath AS target, r.type AS kind, r.reason AS reason, max(r.confidence) AS confidence \
ORDER BY source, target, kind, reason SKIP {offset} LIMIT {page_size}"
    )
}

impl GitNexusCliProvider {
    pub fn new(root: &Path, config: &ProviderConfig) -> Result<Self> {
        if config.page_size == 0 {
            bail!("provider.page_size must be greater than zero");
        }
        let executable =
            std::env::var_os("GITNEXUS_BIN").unwrap_or_else(|| OsString::from(&config.command));
        if executable.is_empty() {
            bail!("GITNEXUS_BIN/provider.command is empty; set it to the GitNexus executable");
        }
        // Resolve path-valued commands against repository root, not the caller's cwd.
        let command_path = Path::new(&executable);
        let executable = if command_path.is_relative() && command_path.components().count() > 1 {
            root.join(command_path).into_os_string()
        } else {
            executable
        };
        Ok(Self {
            executable,
            root: root.to_path_buf(),
            repository: config.repo.clone(),
            page_size: config.page_size,
            edge_types: config.edge_types.clone(),
        })
    }

    /// Where GitNexus keeps this repository's index: `GITNEXUS_STORAGE_PATH`,
    /// else the registry entry named by `provider.repo`, else `<root>/.gitnexus`.
    fn storage_dir(&self) -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("GITNEXUS_STORAGE_PATH") {
            return Some(PathBuf::from(path));
        }
        let Some(repository) = &self.repository else {
            return Some(self.root.join(".gitnexus"));
        };
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        let registry =
            std::fs::read_to_string(Path::new(&home).join(".gitnexus/registry.json")).ok()?;
        let entries: Vec<Value> = serde_json::from_str(&registry).ok()?;
        entries
            .iter()
            .find(|entry| {
                [entry.get("name"), entry.get("path")]
                    .into_iter()
                    .flatten()
                    .any(|value| value.as_str() == Some(repository.as_str()))
            })
            .and_then(|entry| {
                entry
                    .get("storagePath")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .or_else(|| {
                        entry
                            .get("path")
                            .and_then(Value::as_str)
                            .map(|path| Path::new(path).join(".gitnexus"))
                    })
            })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    /// Deliberately argv, not a shell string. No user-controlled Cypher surface.
    fn query_args(&self, query: &str) -> Vec<OsString> {
        let mut args = vec![OsString::from("cypher"), OsString::from(query)];
        if let Some(repository) = &self.repository {
            args.push(OsString::from("--repo"));
            args.push(OsString::from(repository));
        }
        args
    }

    /// Runs a `SKIP`/`LIMIT` query until a short page. Queries must order by a
    /// unique key so pages neither overlap nor skip rows.
    async fn paged<T: PartialEq + Clone>(
        &self,
        what: &str,
        query: impl Fn(usize) -> String,
        parse: impl Fn(&str) -> Result<Vec<T>>,
    ) -> Result<Vec<T>> {
        let mut rows = Vec::new();
        let mut offset: usize = 0;
        let mut previous_first: Option<T> = None;
        loop {
            let stdout = self
                .query(&query(offset))
                .await
                .with_context(|| format!("failed to read {what} page at offset {offset}"))?;
            let mut page = parse(&stdout)?;
            let row_count = page.len(); // Strict wrapper parsing checked equality.
            if row_count > self.page_size {
                return Err(compatibility(
                    "query returned more rows than LIMIT; pagination is unsafe",
                ));
            }
            // A provider that ignores SKIP returns the same full page forever.
            if !page.is_empty() && page.first() == previous_first.as_ref() {
                return Err(compatibility(format!(
                    "{what} page at offset {offset} repeats the previous page; SKIP is not honored"
                )));
            }
            previous_first = page.first().cloned();
            rows.append(&mut page);
            if row_count < self.page_size {
                break;
            }
            offset = offset
                .checked_add(self.page_size)
                .context("GitNexus pagination offset overflow")?;
        }
        Ok(rows)
    }

    async fn query(&self, query: &str) -> Result<String> {
        // GitNexus (Node) exits before draining a piped stdout, truncating
        // results at the 64 KiB pipe buffer. A regular file receives everything.
        let capture =
            tempfile::tempfile().context("cannot create a temporary file for GitNexus output")?;
        let mut command = self.command();
        command
            .args(self.query_args(query))
            .stdout(Stdio::from(
                capture
                    .try_clone()
                    .context("cannot share the GitNexus output file")?,
            ))
            .stderr(Stdio::piped());
        let execute_error = |source| ProviderError::Execute {
            command: self.executable.to_string_lossy().into_owned(),
            source,
        };
        // Not `Command::output()`: tokio would replace the file with a pipe.
        let child = command.spawn().map_err(execute_error)?;
        let result = timeout(Duration::from_secs(300), child.wait_with_output())
            .await
            .map_err(|_| ProviderError::Timeout("cypher query".into()))?
            .map_err(execute_error)?;
        let mut stdout = Vec::new();
        let mut capture = capture;
        capture
            .seek(SeekFrom::Start(0))
            .and_then(|_| capture.read_to_end(&mut stdout))
            .context("cannot read captured GitNexus output")?;
        if !result.status.success() {
            return Err(ProviderError::Command {
                operation: "cypher query".into(),
                status: result.status.to_string(),
                detail: failure_detail(&stdout, &result.stderr),
            }
            .into());
        }
        String::from_utf8(stdout).map_err(|_| compatibility("stdout is not UTF-8"))
    }
}

fn failure_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let detail = if stderr.is_empty() { stdout } else { stderr };
    String::from_utf8_lossy(detail)
        .trim()
        .chars()
        .take(2000)
        .collect()
}

#[async_trait]
impl CodeGraphProvider for GitNexusCliProvider {
    async fn info(&self) -> Result<ProviderInfo> {
        // Version is optional metadata; a failed schema/index probe is never optional.
        let mut version_command = self.command();
        version_command.arg("--version");
        let version = match timeout(Duration::from_secs(15), version_command.output()).await {
            Ok(Ok(output)) if output.status.success() => String::from_utf8(output.stdout)
                .ok()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty()),
            _ => None,
        };
        let stdout = self.query(PROBE).await.context("GitNexus repository is not indexed or File.filePath is unavailable; run `gitnexus analyze --index-only`")?;
        let probe = parse_wrapper(&stdout)?;
        let path = probe.table.column("path")?;
        if probe.row_count > 1 || probe.table.rows.iter().any(|row| !present(&row[path])) {
            return Err(compatibility(
                "File.filePath probe must return at most one nonempty file path",
            ));
        }
        Ok(ProviderInfo {
            provider: "gitnexus".into(),
            version,
            repository: self.repository.clone(),
        })
    }

    async fn fingerprint(&self) -> Result<Option<String>> {
        // Size and modification time of the index metadata and database.
        // Queries do not touch them; `analyze` rewrites both.
        let Some(directory) = self.storage_dir() else {
            return Ok(None);
        };
        let mut parts = Vec::new();
        for name in ["meta.json", "lbug"] {
            let Ok(metadata) = std::fs::metadata(directory.join(name)) else {
                if name == "meta.json" {
                    return Ok(None);
                }
                continue;
            };
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            parts.push(format!("{name}:{}:{modified}", metadata.len()));
        }
        Ok(Some(parts.join(";")))
    }

    async fn dependency_edges(&self) -> Result<Vec<CodeEdge>> {
        self.paged(
            "dependency",
            |offset| edge_query(&self.edge_types, offset, self.page_size),
            parse_edge_page,
        )
        .await
    }

    async fn indexed_files(&self) -> Result<Option<Vec<String>>> {
        self.paged(
            "file",
            |offset| file_query(offset, self.page_size),
            parse_file_page,
        )
        .await
        .map(Some)
    }

    async fn reindex(&self) -> Result<()> {
        let mut command = self.command();
        command.arg("analyze").arg(&self.root).arg("--index-only");
        // Indexing progress goes to stderr so --json stdout stays machine-readable.
        command
            .stdout(Stdio::from(std::io::stderr()))
            .stderr(Stdio::inherit());
        let status = command
            .status()
            .await
            .map_err(|source| ProviderError::Execute {
                command: self.executable.to_string_lossy().into_owned(),
                source,
            })?;
        if !status.success() {
            return Err(ProviderError::Command {
                operation: "analyze --index-only".into(),
                status: status.to_string(),
                detail: "indexing did not complete; inspect GitNexus diagnostics above".into(),
            }
            .into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_wrapper_and_optional_values() {
        let edges = parse_edge_page(include_str!("../../tests/fixtures/imports.json")).unwrap();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].confidence, Some(0.9));
        assert_eq!(edges[0].reason.as_deref(), Some("static|import"));
        assert_eq!(edges[1].confidence, None);
    }
    #[test]
    fn malformed_provider_output_never_becomes_empty_success() {
        for output in [
            "not json",
            "{}",
            "[]",
            "{\"markdown\":\"\",\"row_count\":0}",
            "{\"markdown\":\"| source | target |\\n| --- | --- |\",\"row_count\":1}",
            "{\"markdown\":\"| source | target |\\n| --- | --- |\",\"row_count\":\"0\"}",
        ] {
            assert!(parse_edge_page(output).is_err(), "accepted {output:?}");
        }
    }
    #[test]
    fn columns_can_be_reordered_and_optional_columns_omitted() {
        let output = serde_json::json!({"markdown":"| target | source |\n| --- | --- |\n| b | a |", "row_count": 1});
        let edges = parse_edge_page(&output.to_string()).unwrap();
        assert_eq!(edges[0].from_file, "a");
        assert_eq!(edges[0].confidence, None);
    }
    #[test]
    fn rejects_missing_headers_and_nonfinite_confidence() {
        for table in [
            "| path |\n| --- |",
            "| source | target | confidence |\n| --- | --- | --- |\n| a | b | NaN |",
            "| source | target |\n| --- | --- |\n| a | null |",
        ] {
            let count = table.lines().count() - 2;
            assert!(parse_edge_page(
                &serde_json::json!({"markdown":table,"row_count":count}).to_string()
            )
            .is_err());
        }
    }
    #[test]
    fn argv_preserves_spaces_and_omits_unconfigured_repo() {
        let mut provider = GitNexusCliProvider {
            executable: "gitnexus".into(),
            root: ".".into(),
            repository: None,
            page_size: 2,
            edge_types: vec!["IMPORTS".into()],
        };
        assert_eq!(provider.query_args("a query with spaces").len(), 2);
        provider.repository = Some("repo with spaces; not a shell".into());
        assert_eq!(
            provider.query_args("query")[3],
            "repo with spaces; not a shell"
        );
        let query = edge_query(&["CALLS".into(), "IMPORTS".into()], 1000, 1000);
        assert!(query.contains("r.type IN ['CALLS', 'IMPORTS']"));
        assert!(query.ends_with("ORDER BY source, target, kind, reason SKIP 1000 LIMIT 1000"));
    }
}
