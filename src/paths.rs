//! Repository-relative lexical paths. Never access a provider-supplied file.
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

pub fn normalize_relative(raw: &str) -> Result<String> {
    if raw.contains('\0') || raw.contains('\n') || raw.contains('\r') {
        bail!("path contains an unsupported NUL/newline: {raw:?}");
    }
    let replaced = raw.replace('\\', "/");
    if replaced.starts_with('/') || replaced.as_bytes().get(1) == Some(&b':') {
        bail!("path `{raw}` must be repository-relative, not absolute");
    }
    let mut parts = Vec::new();
    for part in replaced.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    bail!("path `{raw}` escapes the repository root");
                }
            }
            p => parts.push(p),
        }
    }
    Ok(parts.join("/"))
}

pub fn relative_file(root: &Path, file: &Path) -> Result<String> {
    let relative = file.strip_prefix(root).with_context(|| {
        format!(
            "file {} is outside repository {}",
            file.display(),
            root.display()
        )
    })?;
    let raw = relative
        .to_str()
        .context("non-UTF-8 repository path; rename it to a UTF-8 path")?;
    let normalized = normalize_relative(raw)?;
    if normalized.is_empty() {
        bail!("expected a file path, got repository root");
    }
    Ok(normalized)
}

/// Absolute provider paths are accepted only when they are under this root.
/// Bad or outside paths become compiler diagnostics, not fabricated memberships.
pub fn provider_path(root: &Path, raw: &str) -> Result<String> {
    let path = portable_absolute(raw);
    let root_string = portable_absolute(root.to_str().context("repository root is not UTF-8")?);
    let root_prefix = format!("{}/", root_string.trim_end_matches('/'));
    // Windows canonicalize can add a verbatim prefix and change drive casing.
    // Only the absolute root prefix is compared case-insensitively; repository
    // file identities keep their authored casing for globs and diagnostics.
    let windows_root =
        root_string.as_bytes().get(1) == Some(&b':') || root_string.starts_with("//");
    let relative = if windows_root
        && path
            .get(..root_prefix.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&root_prefix))
    {
        &path[root_prefix.len()..]
    } else {
        path.strip_prefix(&root_prefix).unwrap_or(&path)
    };
    let normalized = normalize_relative(relative)?;
    if normalized.is_empty() {
        bail!("provider returned an empty file path");
    }
    Ok(normalized)
}

fn portable_absolute(raw: &str) -> String {
    let normalized = raw.replace('\\', "/");
    if let Some(unc) = normalized.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else {
        normalized
            .strip_prefix("//?/")
            .unwrap_or(&normalized)
            .to_owned()
    }
}

pub fn locate_repository(explicit: Option<&Path>, start: &Path) -> Result<PathBuf> {
    if let Some(root) = explicit {
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot open --root {}", root.display()))?;
        if !root.is_dir() {
            bail!("--root {} is not a directory", root.display());
        }
        return Ok(root);
    }
    let start = start
        .canonicalize()
        .context("cannot resolve current directory")?;
    for directory in start.ancestors() {
        // Worktrees have a .git file instead of a directory.
        if directory.join(".git").exists() {
            return Ok(directory.to_path_buf());
        }
    }
    bail!(
        "no Git repository found above {}; run from a repository or pass --root <directory>",
        start.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_separators_and_dot_segments() {
        assert_eq!(normalize_relative(r".\src\a\..\b.rs").unwrap(), "src/b.rs");
        assert!(normalize_relative("../outside").is_err());
        assert!(normalize_relative("C:\\src\\a.rs").is_err());
        assert!(normalize_relative("/tmp/a").is_err());
    }

    #[test]
    fn absolute_provider_path_requires_repository_boundary() {
        assert_eq!(
            provider_path(Path::new("/work/repo"), "/work/repo/src/a").unwrap(),
            "src/a"
        );
        assert!(provider_path(Path::new("/work/repo"), "/work/repository/a").is_err());
    }

    #[test]
    fn windows_verbatim_roots_and_drive_case_are_compatible() {
        let root = Path::new(r"\\?\C:\Work\repo");
        assert_eq!(
            provider_path(root, r"c:\Work\repo\src\a.rs").unwrap(),
            "src/a.rs"
        );
        let unc = Path::new(r"\\?\UNC\server\share\repo");
        assert_eq!(
            provider_path(unc, r"\\server\share\repo\src\a.rs").unwrap(),
            "src/a.rs"
        );
    }

    #[test]
    fn worktree_marker_and_explicit_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".git"), "gitdir: elsewhere").unwrap();
        let nested = temp.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            locate_repository(None, &nested).unwrap(),
            temp.path().canonicalize().unwrap()
        );
    }
}
