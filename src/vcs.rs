//! The Git facts baselines need: the commit a baseline was taken at, and which
//! files moved since. Moving legacy code is usually the first refactoring step;
//! without this, every accepted observation in a moved file became "new" and
//! failed `check`. Like GitNexus, this looks at the working tree, committed or
//! not: renames are detected between the baseline commit and the files on disk,
//! including untracked ones.
use anyhow::{bail, Context, Result};
use std::{collections::BTreeMap, path::Path, process::Command};

fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(root).env_remove("GIT_INDEX_FILE");
    command
}

fn run(command: &mut Command, what: &str) -> Result<Vec<u8>> {
    let output = command
        .output()
        .with_context(|| format!("cannot run git to {what}"))?;
    if !output.status.success() {
        bail!(
            "git failed to {what}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

/// `HEAD`, or `None` outside a Git repository or before the first commit.
pub fn head_commit(root: &Path) -> Option<String> {
    let stdout = run(
        git(root).args(["rev-parse", "--verify", "-q", "HEAD"]),
        "read HEAD",
    )
    .ok()?;
    let commit = String::from_utf8(stdout).ok()?.trim().to_owned();
    (!commit.is_empty()).then_some(commit)
}

/// Files renamed between `commit` and the working tree, old path -> new path,
/// relative to `root`. Works on a copy of the Git index with every working-tree
/// file marked intent-to-add, so untracked moves count, the user's index is
/// untouched and no objects are written.
pub fn renames_since(root: &Path, commit: &str) -> Result<BTreeMap<String, String>> {
    if !commit.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("baseline commit `{commit}` is not a commit id");
    }
    run(
        git(root).args(["cat-file", "-e", &format!("{commit}^{{commit}}")]),
        &format!("find baseline commit {commit} (a shallow clone may lack it; fetch more history)"),
    )?;
    let index = String::from_utf8(run(
        git(root).args(["rev-parse", "--path-format=absolute", "--git-path", "index"]),
        "locate the Git index",
    )?)
    .context("Git index path is not UTF-8")?;
    let temporary =
        tempfile::NamedTempFile::new().context("cannot create a temporary Git index")?;
    match std::fs::copy(index.trim(), temporary.path()) {
        Ok(_) => {}
        // A repository without an index yet: start from an empty one.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::remove_file(temporary.path())?;
        }
        Err(error) => return Err(error).context("cannot copy the Git index"),
    }
    let with_index = |command: &mut Command| {
        command.env("GIT_INDEX_FILE", temporary.path());
    };
    let mut add = git(root);
    with_index(&mut add);
    run(
        add.args(["add", "--all", "--intent-to-add", "--", "."]),
        "list working-tree files",
    )?;
    let mut diff = git(root);
    with_index(&mut diff);
    let stdout = run(
        diff.args([
            "-c",
            "core.quotePath=false",
            "diff",
            "--relative",
            "--find-renames",
            "--diff-filter=R",
            "--name-status",
            "-z",
            commit,
        ]),
        "detect renamed files",
    )?;
    let text = String::from_utf8(stdout).context("git reported a non-UTF-8 path")?;
    let mut fields = text.split('\0').filter(|field| !field.is_empty());
    let mut renames = BTreeMap::new();
    while let Some(status) = fields.next() {
        let (Some(old), Some(new)) = (fields.next(), fields.next()) else {
            bail!("unexpected `git diff --name-status` output after `{status}`");
        };
        if !status.starts_with('R') {
            bail!("unexpected `git diff` status `{status}`");
        }
        renames.insert(old.to_owned(), new.to_owned());
    }
    Ok(renames)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_ok(root: &Path, args: &[&str]) {
        let status = git(root)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn content(tag: &str) -> String {
        (0..40).map(|line| format!("{tag} line {line}\n")).collect()
    }

    #[test]
    fn detects_committed_staged_and_untracked_moves_without_touching_the_index() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git_ok(root, &["init", "-q"]);
        std::fs::create_dir_all(root.join("lib")).unwrap();
        for name in ["a", "b", "c", "d"] {
            std::fs::write(root.join(format!("lib/{name}.rb")), content(name)).unwrap();
        }
        git_ok(root, &["add", "-A"]);
        git_ok(root, &["commit", "-qm", "base"]);
        let commit = head_commit(root).unwrap();

        git_ok(root, &["mv", "lib/a.rb", "lib/a2.rb"]);
        git_ok(root, &["commit", "-qm", "move a"]);
        git_ok(root, &["mv", "lib/b.rb", "lib/b2.rb"]);
        std::fs::create_dir_all(root.join("app/moved")).unwrap();
        // Untracked, and edited while moving.
        std::fs::rename(root.join("lib/c.rb"), root.join("app/moved/c.rb")).unwrap();
        let mut edited = content("c");
        edited.push_str("extra\n");
        std::fs::write(root.join("app/moved/c.rb"), edited).unwrap();
        let status_before = run(git(root).args(["status", "--porcelain"]), "status").unwrap();

        let renames = renames_since(root, &commit).unwrap();
        assert_eq!(
            renames,
            BTreeMap::from([
                ("lib/a.rb".to_owned(), "lib/a2.rb".to_owned()),
                ("lib/b.rb".to_owned(), "lib/b2.rb".to_owned()),
                ("lib/c.rb".to_owned(), "app/moved/c.rb".to_owned()),
            ])
        );
        let status_after = run(git(root).args(["status", "--porcelain"]), "status").unwrap();
        assert_eq!(status_before, status_after, "the user's index changed");
        assert!(renames_since(root, "0123456789abcdef0123456789abcdef01234567").is_err());
        assert!(renames_since(root, "HEAD; rm -rf /").is_err());
    }

    #[test]
    fn no_commit_outside_a_repository() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(head_commit(temp.path()), None);
    }
}
