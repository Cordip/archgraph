//! Read-only access to the source of mapped files, for the UI's code viewer.
//!
//! The scope is deliberately narrow: a file is served only when the current
//! IR maps it to an architecture node, and only as UTF-8 text of at most
//! `SOURCE_SIZE_LIMIT` bytes. Every check happens before anything is read,
//! and the path is resolved on disk and checked against the canonical
//! repository root, so neither `..`, a symlink nor a file that changed into a
//! directory since the compile can reach anything else.
use crate::{model::ArchitectureIr, paths::normalize_relative};
use serde::Serialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// Larger files are refused: the viewer is for reading code, not data.
pub const SOURCE_SIZE_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceFile {
    pub path: String,
    /// The node owning the file in the IR the check used.
    pub node: String,
    /// Absent when the caller already has this content (`if_hash`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub line_count: usize,
    pub bytes: usize,
    /// FNV-1a 64 of the bytes, in hex: tells a changed file from an
    /// unchanged one, nothing more. It is not a security hash.
    pub hash: String,
    pub unchanged: bool,
}

/// Why a file is not served. `reason` is a stable code the UI and the tests
/// match on; `message` says what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub reason: &'static str,
    pub message: String,
}

fn refuse(status: u16, reason: &'static str, message: impl Into<String>) -> Refusal {
    Refusal {
        status,
        reason,
        message: message.into(),
    }
}

/// `root` must be canonical. `if_hash`: the hash the caller already shows;
/// when it still matches, the text is left out of the answer.
pub fn read_mapped(
    ir: &ArchitectureIr,
    root: &Path,
    raw: &str,
    if_hash: Option<&str>,
) -> Result<SourceFile, Refusal> {
    let path = checked_path(raw)?;
    let node = mapped_node(ir, &path)?;
    let file = resolve(root, &path)?;
    let bytes = read_limited(&file, &path)?;
    let text = decode(bytes, &path)?;
    let hash = format!("{:016x}", fnv1a(text.as_bytes()));
    let unchanged = if_hash == Some(hash.as_str());
    Ok(SourceFile {
        line_count: line_count(&text),
        bytes: text.len(),
        path,
        node,
        text: if unchanged { None } else { Some(text) },
        hash,
        unchanged,
    })
}

/// A repository-relative path exactly as the IR spells it: no leading
/// slash or drive, no `.` or `..` segment, no backslash, no NUL.
fn checked_path(raw: &str) -> Result<String, Refusal> {
    if raw.is_empty() {
        return Err(refuse(
            400,
            "empty",
            "pass the repository-relative path of a mapped file in `path`",
        ));
    }
    if raw.starts_with('/') || raw.starts_with('\\') || raw.as_bytes().get(1) == Some(&b':') {
        return Err(refuse(
            400,
            "absolute",
            format!("`{raw}` is absolute; pass a repository-relative path"),
        ));
    }
    if raw.split(['/', '\\']).any(|part| part == "..") {
        return Err(refuse(
            400,
            "parent",
            format!("`{raw}` contains `..`; pass the file's own repository-relative path"),
        ));
    }
    match normalize_relative(raw) {
        Ok(normalized) if normalized == raw => Ok(normalized),
        Ok(normalized) => Err(refuse(
            400,
            "not_normalized",
            format!("`{raw}` is not in normal form; use `{normalized}`"),
        )),
        Err(error) => Err(refuse(400, "invalid", error.to_string())),
    }
}

/// Files the IR maps to a node; excluded, unassigned, ambiguous and unknown
/// files are refused alike, whatever is on disk.
fn mapped_node(ir: &ArchitectureIr, path: &str) -> Result<String, Refusal> {
    let found = ir
        .files
        .binary_search_by(|file| file.path.as_str().cmp(path))
        .ok()
        .map(|index| &ir.files[index]);
    match found {
        Some(file) => file.node.clone().ok_or_else(|| {
            refuse(
                403,
                "unmapped",
                format!("`{path}` is not mapped to a single architecture node (unassigned or ambiguous); only mapped files are served"),
            )
        }),
        None => Err(refuse(
            403,
            "unmapped",
            format!("`{path}` is not a mapped file of this architecture (excluded, outside the source roots or unknown); only mapped files are served"),
        )),
    }
}

/// The file on disk, after the checks that the filesystem can defeat: the
/// file itself must not be a symlink, and its resolved location must stay
/// inside the root (a directory on the way may have become a symlink).
fn resolve(root: &Path, path: &str) -> Result<PathBuf, Refusal> {
    let joined = root.join(path);
    let metadata = std::fs::symlink_metadata(&joined).map_err(|_| {
        refuse(
            404,
            "missing",
            format!("`{path}` no longer exists; the view reloads after the next index"),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(refuse(
            403,
            "symlink",
            format!("`{path}` is a symlink; only regular files are served"),
        ));
    }
    let canonical = joined
        .canonicalize()
        .map_err(|error| refuse(404, "missing", format!("cannot resolve `{path}`: {error}")))?;
    if !canonical.starts_with(root) {
        return Err(refuse(
            403,
            "outside",
            format!("`{path}` resolves outside the repository; it is not served"),
        ));
    }
    let metadata = std::fs::metadata(&canonical)
        .map_err(|error| refuse(404, "missing", format!("cannot read `{path}`: {error}")))?;
    if !metadata.is_file() {
        return Err(refuse(
            403,
            "not_file",
            format!("`{path}` is a directory or special file, not a regular file"),
        ));
    }
    Ok(canonical)
}

/// Reads at most one byte past the limit: a larger file is refused without
/// being read, and so is one that grew since its metadata was checked.
fn read_limited(file: &Path, path: &str) -> Result<Vec<u8>, Refusal> {
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .and_then(|handle| handle.take(SOURCE_SIZE_LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|error| refuse(404, "missing", format!("cannot read `{path}`: {error}")))?;
    if bytes.len() as u64 > SOURCE_SIZE_LIMIT {
        return Err(refuse(
            413,
            "too_large",
            format!("`{path}` is larger than {SOURCE_SIZE_LIMIT} bytes (1 MiB), the most the viewer shows; open it in an editor"),
        ));
    }
    Ok(bytes)
}

/// Text only: a NUL byte marks a binary file, and anything else must be
/// valid UTF-8 (nothing is guessed or replaced).
fn decode(bytes: Vec<u8>, path: &str) -> Result<String, Refusal> {
    if bytes.contains(&0) {
        return Err(refuse(
            415,
            "binary",
            format!("`{path}` looks binary (it contains NUL bytes); only text is shown"),
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        refuse(
            415,
            "not_utf8",
            format!(
                "`{path}` is not valid UTF-8 (at byte {}); only UTF-8 text is shown",
                error.utf8_error().valid_up_to()
            ),
        )
    })
}

/// Lines as an editor numbers them: a final newline ends the last line
/// rather than starting another.
pub fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.bytes().filter(|&byte| byte == b'\n').count() + usize::from(!text.ends_with('\n'))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_like_an_editor() {
        assert_eq!(line_count(""), 0);
        assert_eq!(line_count("a"), 1);
        assert_eq!(line_count("a\n"), 1);
        assert_eq!(line_count("a\nb"), 2);
        assert_eq!(line_count("a\r\nb\r\n"), 2);
        assert_eq!(line_count("\n\n"), 2);
    }

    #[test]
    fn the_hash_is_fnv1a_64() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
