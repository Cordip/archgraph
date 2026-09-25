//! Unix subprocess tests use a Python fixture to exercise real argv/cwd/paging.
//! Production ArchGraph and all core tests have no Python dependency.
#![cfg(unix)]

use serde_json::Value;
use std::{
    os::unix::fs::PermissionsExt,
    process::{Command, Output},
};

const YAML: &str = r#"version: 1
project: {name: cli-fixture, root: app, source_roots: [src]}
provider: {kind: gitnexus, command: intentionally-missing-fallback, page_size: 1}
nodes:
  app: {}
  app.a: {maps: ["src/a*.rs"]}
  app.b: {maps: ["src/b.rs"]}
  app.c: {}
rules:
  - {id: denied, kind: deny_dependency, from: app.a, to: app.b}
"#;

struct Fixture {
    root: tempfile::TempDir,
    executable: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        assert!(
            Command::new("python3").arg("--version").output().is_ok(),
            "CLI subprocess fixtures require python3; core tests do not"
        );
        let root = tempfile::Builder::new()
            .prefix("archgraph repo ")
            .tempdir()
            .unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/a.rs"), "").unwrap();
        std::fs::write(root.path().join("src/b.rs"), "").unwrap();
        std::fs::write(root.path().join("src/a2.rs"), "").unwrap();
        std::fs::write(root.path().join("architecture.yaml"), YAML).unwrap();
        let executable = root.path().join("provider with spaces.py");
        std::fs::write(&executable, include_str!("fixtures/fake_gitnexus.py")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { root, executable }
    }
    fn run(&self, arguments: &[&str], mode: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_archgraph"))
            .arg("--root")
            .arg(self.root.path())
            .args(arguments)
            .env("GITNEXUS_BIN", &self.executable)
            .env("ARCHGRAPH_FAKE_MODE", mode)
            .env("ARCHGRAPH_FAKE_LOG", self.root.path().join("provider.log"))
            .output()
            .unwrap()
    }
    fn logs(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.path().join("provider.log"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[test]
fn check_exit_zero_for_clean_two_for_violations_and_scope_filtering() {
    let fixture = Fixture::new();
    assert_eq!(fixture.run(&["check"], "clean").status.code(), Some(0));
    assert_eq!(fixture.run(&["check"], "violation").status.code(), Some(2));
    assert_eq!(
        fixture.run(&["check", "app.a"], "violation").status.code(),
        Some(2)
    );
    assert_eq!(
        fixture.run(&["check", "app.c"], "violation").status.code(),
        Some(0)
    );
    assert_eq!(
        fixture.run(&["compile"], "violation").status.code(),
        Some(0)
    );
}

#[test]
fn baseline_accepts_existing_violations_and_fails_only_on_new_ones() {
    let fixture = Fixture::new();
    let text = |output: &Output| String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(fixture.run(&["check"], "violation").status.code(), Some(2));
    let written = fixture.run(&["baseline"], "violation");
    assert_eq!(
        written.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    assert!(fixture
        .root
        .path()
        .join("architecture.baseline.json")
        .exists());

    let accepted = fixture.run(&["check"], "violation");
    assert_eq!(accepted.status.code(), Some(0), "{}", text(&accepted));
    assert!(text(&accepted).contains("1 accepted observation(s), 0 fixed since"));

    let grown = fixture.run(&["check"], "violation_grown");
    assert_eq!(grown.status.code(), Some(2));
    assert!(
        text(&grown).contains("New since baseline (1):\n    src/a2.rs -> src/b.rs [IMPORTS]"),
        "{}",
        text(&grown)
    );
    let json: Value =
        serde_json::from_slice(&fixture.run(&["check", "--json"], "violation_grown").stdout)
            .unwrap();
    assert_eq!(json["violation_count"], 1);
    assert_eq!(
        json["violations"][0]["new_observations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(json["baseline"]["accepted_observations"], 1);

    let fixed = fixture.run(&["check"], "clean");
    assert_eq!(fixed.status.code(), Some(0));
    assert!(text(&fixed).contains("1 fixed since"), "{}", text(&fixed));
    assert_eq!(
        fixture
            .run(&["check", "--no-baseline"], "violation")
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn an_index_rewritten_while_compiling_fails_instead_of_mixing_graphs() {
    let fixture = Fixture::new();
    let index = fixture.root.path().join(".gitnexus");
    std::fs::create_dir(&index).unwrap();
    std::fs::write(index.join("meta.json"), "{}").unwrap();
    assert_eq!(fixture.run(&["check"], "violation").status.code(), Some(2));
    let output = fixture.run(&["check", "--json"], "index_rewrite");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("index changed while compiling"));
}

#[test]
fn low_coverage_policy_error_makes_check_unverified() {
    let fixture = Fixture::new();
    let with_policy = |policy: &str| {
        let yaml = YAML.replace(
            "nodes:\n",
            &format!("policies: {{low_coverage: {policy}, min_observed_ratio: 0.9}}\nnodes:\n"),
        );
        std::fs::write(fixture.root.path().join("architecture.yaml"), yaml).unwrap();
    };
    // app.a maps src/a.rs and src/a2.rs; only a.rs has an observed dependency.
    with_policy("error");
    let output = fixture.run(&["check"], "violation");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Not verified") && stdout.contains("only 1 of 2 files under `app.a`"),
        "{stdout}"
    );
    let json: Value =
        serde_json::from_slice(&fixture.run(&["check", "--json"], "violation").stdout).unwrap();
    assert_eq!(json["coverage_failures"][0]["node"], "app.a");
    assert_eq!(
        fixture.run(&["check", "app.c"], "violation").status.code(),
        Some(0)
    );
    with_policy("warn");
    assert_eq!(fixture.run(&["check"], "violation").status.code(), Some(2));
}

#[test]
fn closed_stdout_exits_quietly_instead_of_panicking() {
    let fixture = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_archgraph"))
        .arg("--root")
        .arg(fixture.root.path())
        .arg("check")
        .env("GITNEXUS_BIN", &fixture.executable)
        .env("ARCHGRAPH_FAKE_MODE", "violation")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // like `archgraph check | head -0`
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(output.status.code(), Some(141));
}

#[test]
fn text_output_names_architecture_ids_and_evidence_reasons() {
    let fixture = Fixture::new();
    let show = String::from_utf8(fixture.run(&["show"], "violation").stdout).unwrap();
    assert!(show.contains("  app.a -> app.b  IMPORTS"), "{show}");
    assert!(
        !show.contains("node:"),
        "internal projection IDs leaked: {show}"
    );
    let check = String::from_utf8(fixture.run(&["check"], "violation").stdout).unwrap();
    assert!(
        check.contains("src/a.rs -> src/b.rs  (static|import, confidence 1)"),
        "{check}"
    );
}

#[test]
fn provider_failures_and_invalid_configuration_exit_one() {
    let fixture = Fixture::new();
    for mode in [
        "unindexed",
        "malformed",
        "query_failure",
        "bad_count",
        "bad_confidence",
    ] {
        let output = fixture.run(&["check", "--json"], mode);
        assert_eq!(output.status.code(), Some(1), "mode {mode}");
        assert!(
            output.stdout.is_empty(),
            "provider failure emitted success JSON"
        );
    }
    std::fs::write(
        fixture.root.path().join("architecture.yaml"),
        "invalid: [yaml",
    )
    .unwrap();
    assert_eq!(fixture.run(&["check"], "clean").status.code(), Some(1));
}

#[test]
fn cli_usage_errors_do_not_collide_with_violation_exit_code() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .run(&["check", "--unknown-option"], "clean")
            .status
            .code(),
        Some(1)
    );
    assert_eq!(fixture.run(&["context"], "clean").status.code(), Some(1));
    assert_eq!(fixture.run(&["--help"], "clean").status.code(), Some(0));
}

#[test]
fn json_show_context_and_check_are_machine_readable() {
    let fixture = Fixture::new();
    for arguments in [
        &["show", "--format", "json"][..],
        &["context", "app", "--json"][..],
        &["check", "--json"][..],
    ] {
        let output = fixture.run(arguments, "violation");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value.is_object());
    }
    let mermaid = fixture.run(&["show", "--format", "mermaid"], "violation");
    assert!(String::from_utf8(mermaid.stdout)
        .unwrap()
        .starts_with("flowchart LR"));
}

#[test]
fn provider_output_larger_than_a_pipe_buffer_is_not_truncated() {
    let fixture = Fixture::new();
    let output = fixture.run(&["compile", "--json"], "node_like_large");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["stats"]["observed_edge_count"], 1);
}

#[test]
fn pagination_uses_argv_cwd_and_a_final_empty_page() {
    let fixture = Fixture::new();
    let output = fixture.run(&["compile", "--json"], "cycle");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["stats"]["observed_edge_count"], 2);
    let logs = fixture.logs();
    let queries: Vec<_> = logs
        .iter()
        .filter(|log| {
            log["args"][1]
                .as_str()
                .is_some_and(|q| q.contains(" SKIP "))
        })
        .collect();
    assert_eq!(queries.len(), 3);
    for (offset, query) in queries.into_iter().enumerate() {
        assert!(query["args"][1]
            .as_str()
            .unwrap()
            .ends_with(&format!("SKIP {offset} LIMIT 1")));
        assert_eq!(query["args"].as_array().unwrap().len(), 2);
        assert_eq!(
            query["cwd"].as_str().unwrap(),
            fixture
                .root
                .path()
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
}

#[test]
fn reindex_is_incremental_repo_is_optional_and_progress_does_not_corrupt_json() {
    let fixture = Fixture::new();
    let yaml = YAML.replace(
        "page_size: 1",
        "page_size: 1, repo: 'repo with spaces; data not shell'",
    );
    std::fs::write(fixture.root.path().join("architecture.yaml"), yaml).unwrap();
    let output = fixture.run(&["compile", "--reindex", "--json"], "clean");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(serde_json::from_slice::<Value>(&output.stdout).is_ok());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture-index-progress"));
    let logs = fixture.logs();
    assert_eq!(logs[0]["args"][0], "analyze");
    assert_eq!(logs[0]["args"][2], "--index-only");
    assert_eq!(logs[0]["args"].as_array().unwrap().len(), 3);
    for query in logs.iter().filter(|log| log["args"][0] == "cypher") {
        assert_eq!(query["args"][2], "--repo");
        assert_eq!(query["args"][3], "repo with spaces; data not shell");
    }
}
