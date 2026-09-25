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
    // The fixture changes its answers without changing the index; a cached
    // result would hide the rewrite.
    let output = fixture.run(&["check", "--json", "--no-cache"], "index_rewrite");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("index changed while compiling"));
    // A query that breaks on a half-written index is blamed on the rewrite,
    // not reported as an incompatible GitNexus.
    let output = fixture.run(&["check", "--no-cache"], "index_rewrite_torn");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("index changed while compiling"), "{stderr}");
    assert!(
        stderr.contains("unsupported GitNexus cypher output"),
        "{stderr}"
    );
}

#[test]
fn provider_results_are_reused_until_the_index_changes() {
    let fixture = Fixture::new();
    let index = fixture.root.path().join(".gitnexus");
    std::fs::create_dir(&index).unwrap();
    std::fs::write(index.join("meta.json"), "{}").unwrap();
    let queries = || {
        fixture
            .logs()
            .iter()
            .filter(|entry| entry["args"][0] == "cypher")
            .count()
    };
    let first = fixture.run(&["compile"], "violation");
    assert!(first.status.success());
    let after_first = queries();
    assert!(after_first > 0);
    assert!(!String::from_utf8_lossy(&first.stdout).contains("cached"));
    let ir = std::fs::read(fixture.root.path().join(".archgraph/architecture.ir.json")).unwrap();

    let second = fixture.run(&["compile"], "violation");
    assert!(String::from_utf8_lossy(&second.stdout).contains("(cached: index unchanged)"));
    assert_eq!(
        queries(),
        after_first,
        "an unchanged index was queried again"
    );
    let cached_ir =
        std::fs::read(fixture.root.path().join(".archgraph/architecture.ir.json")).unwrap();
    assert_eq!(ir, cached_ir, "the cache changed the IR");
    // Configuration is not cached: a new rule applies to cached observations.
    std::fs::write(
        fixture.root.path().join("architecture.yaml"),
        YAML.replace("to: app.b}", "to: app.c}"),
    )
    .unwrap();
    assert_eq!(fixture.run(&["check"], "violation").status.code(), Some(0));
    assert_eq!(queries(), after_first);
    std::fs::write(fixture.root.path().join("architecture.yaml"), YAML).unwrap();

    assert_eq!(
        fixture
            .run(&["check", "--no-cache"], "violation")
            .status
            .code(),
        Some(2)
    );
    assert!(queries() > after_first);
    let before_reindex = queries();
    std::fs::write(index.join("meta.json"), "{\"reindexed\": true}").unwrap();
    assert_eq!(fixture.run(&["check"], "violation").status.code(), Some(2));
    assert!(
        queries() > before_reindex,
        "a changed index was not queried"
    );
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
fn unindexed_files_and_out_of_scope_edges_are_told_apart() {
    let fixture = Fixture::new();
    let output = fixture.run(&["compile", "--json"], "out_of_scope");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    // src/a2.rs exists and is mapped but the fake index does not list it.
    assert_eq!(value["stats"]["unindexed_file_count"], 1);
    assert_eq!(value["diagnostics"]["unindexed_files"][0], "src/a2.rs");
    assert!(String::from_utf8_lossy(&output.stderr).contains("not in the code-graph index at all"));
    // An edge into vendor/, outside source_roots, is expected, not an anomaly.
    assert_eq!(value["stats"]["out_of_scope_edge_count"], 1);
    assert_eq!(
        value["diagnostics"]["provider_anomalies"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn a_provider_that_ignores_skip_fails_instead_of_looping() {
    let fixture = Fixture::new();
    let output = fixture.run(&["compile"], "ignores_skip");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("SKIP is not honored"));
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
                .is_some_and(|q| q.contains("CodeRelation") && q.contains(" SKIP "))
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

#[test]
fn full_reindex_rebuilds_and_the_flag_does_not_swallow_the_node() {
    let fixture = Fixture::new();
    let output = fixture.run(&["compile", "--reindex=full"], "clean");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let analyze = |logs: &[Value]| {
        logs.iter()
            .filter(|log| log["args"][0] == "analyze")
            .map(|log| log["args"].as_array().unwrap()[3..].to_vec())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        analyze(&fixture.logs()),
        vec![vec![
            Value::from("--force"),
            Value::from("--no-parse-cache")
        ]]
    );
    // `--reindex` takes its value only with `=`, so `app.a` stays the node.
    let scoped = fixture.run(&["check", "--reindex", "app.c"], "violation");
    assert_eq!(
        scoped.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&scoped.stderr)
    );
    assert_eq!(
        analyze(&fixture.logs()).last().unwrap(),
        &Vec::<Value>::new()
    );
}

#[test]
fn init_suggest_drafts_nodes_with_coverage_and_the_draft_checks() {
    let fixture = Fixture::new();
    for directory in ["src/x", "src/y"] {
        std::fs::create_dir_all(fixture.root.path().join(directory)).unwrap();
        for index in 0..6 {
            std::fs::write(
                fixture.root.path().join(format!("{directory}/m{index}.rs")),
                "",
            )
            .unwrap();
        }
    }
    let yaml = fixture.root.path().join("architecture.yaml");
    let kept = fixture.run(&["init", "--suggest"], "violation");
    assert!(String::from_utf8_lossy(&kept.stdout).contains("Keeping existing"));
    assert_eq!(std::fs::read_to_string(&yaml).unwrap(), YAML);

    let output = fixture.run(&["init", "--suggest", "--force"], "violation");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let draft = std::fs::read_to_string(&yaml).unwrap();
    assert!(draft.contains("maps: [\"src/x/**\"]"), "{draft}");
    assert!(draft.contains("maps: [\"src/y/**\"]"), "{draft}");
    // src/a.rs -> src/b.rs is the only observed dependency; src/a2.rs is not indexed.
    assert!(
        draft.contains("# 2 of 16 files have an observed dependency (12%)"),
        "{draft}"
    );
    assert!(draft.contains("kind: no_cycles"), "{draft}");
    assert_eq!(fixture.run(&["check"], "violation").status.code(), Some(0));

    let output = fixture.run(&["init", "--suggest", "--force"], "unindexed");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("repository is not indexed"));
    let draft = std::fs::read_to_string(&yaml).unwrap();
    assert!(
        draft.contains("# No coverage comments: no readable GitNexus index."),
        "{draft}"
    );
    assert!(!draft.contains("observed dependency ("), "{draft}");
}

#[test]
fn check_prints_a_cycle_as_members_cut_and_layers() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.root.path().join("architecture.yaml"),
        YAML.replace(
            "  - {id: denied, kind: deny_dependency, from: app.a, to: app.b}",
            "  - {id: layers, kind: no_cycles, within: app}",
        ),
    )
    .unwrap();
    let output = fixture.run(&["check"], "cycle");
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stdout);
    let expected = "[layers] cycle among immediate children of `app`\n  Members: app.a, app.b\n  Cheapest cut: 1 of 2 participating dependencies, most significant first:\n";
    assert!(text.contains(expected), "{text}");
    assert!(
        text.contains("  Layers after the cut, upper to lower:\n    app."),
        "{text}"
    );
    // The one-line summary stays in JSON, not in the text report.
    assert!(!text.contains("SCC"), "{text}");
}

const CSS_YAML: &str = r#"version: 1
project: {name: css-fixture, root: app, source_roots: [src]}
provider: {kind: gitnexus, command: intentionally-missing-fallback, edge_types: [IMPORTS, USES_CLASS], css: true}
policies: {unassigned_files: ignore}
nodes:
  app: {}
  app.ui: {maps: ["src/app.tsx"]}
  app.menu: {maps: ["src/menu.tsx"]}
  app.styles: {maps: ["src/*.css"]}
rules:
  - {id: menu-unstyled, kind: deny_dependency, from: app.menu, to: app.styles, edge_types: [USES_CLASS]}
"#;

#[test]
fn css_classes_become_edges_and_the_styles_report() {
    let fixture = Fixture::new();
    let root = fixture.root.path();
    std::fs::write(root.join("architecture.yaml"), CSS_YAML).unwrap();
    std::fs::write(
        root.join("src/app.tsx"),
        "import './styles.css'\nexport const App = () => <div className=\"panel missing\" />\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/menu.tsx"),
        "export const Menu = () => <nav className={`panel tone-${level}`} />\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/styles.css"),
        ".panel { margin: 0 }\n.unused { margin: 0 }\n.tone-high { color: red }\n",
    )
    .unwrap();

    // The fake provider fails any query other than IMPORTS, so USES_CLASS
    // must never reach GitNexus.
    let check = fixture.run(&["check"], "clean");
    let text = String::from_utf8_lossy(&check.stdout);
    assert_eq!(check.status.code(), Some(2), "{text}");
    assert!(
        text.contains("src/menu.tsx -> src/styles.css  (classname-expression, confidence 0.8)"),
        "{text}"
    );
    let ir: Value = serde_json::from_slice(
        &std::fs::read(root.join(".archgraph/architecture.ir.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(ir["stats"]["stylesheet_row_count"], 3);
    let evidence = ir["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edge| edge["from"] == "app.ui" && edge["kind"] == "IMPORTS")
        .unwrap();
    assert_eq!(evidence["evidence"][0]["reason"], "css-import");

    let styles = fixture.run(&["styles"], "clean");
    let report = String::from_utf8_lossy(&styles.stdout);
    assert_eq!(styles.status.code(), Some(0), "{report}");
    for expected in [
        "Classes: 1 used, 1 undefined, 1 unused, 1 possibly used dynamically",
        "  .missing  src/app.tsx:2",
        "  .unused  src/styles.css:2",
        "  .tone-high  src/styles.css:3 (`tone-…` at src/menu.tsx:1)",
        "  .panel  app.menu, app.ui",
    ] {
        assert!(
            report.contains(expected),
            "missing {expected:?} in\n{report}"
        );
    }
    let scoped: Value = serde_json::from_slice(
        &fixture
            .run(&["styles", "app.menu", "--json"], "clean")
            .stdout,
    )
    .unwrap();
    let names: Vec<&str> = scoped["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|class| class["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["panel"]);
    assert_eq!(scoped["dynamic_uses"][0]["prefix"], "tone-");
    assert_eq!(
        scoped["shared"],
        serde_json::json!([{"class": "panel", "nodes": ["app.menu", "app.ui"]}])
    );

    // Without `css: true`, USES_CLASS could never be observed.
    std::fs::write(
        root.join("architecture.yaml"),
        CSS_YAML.replace(", css: true", ""),
    )
    .unwrap();
    let invalid = fixture.run(&["check"], "clean");
    assert_eq!(invalid.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("only `provider.css: true` observes"));
    std::fs::write(
        root.join("architecture.yaml"),
        CSS_YAML
            .replace(", css: true", "")
            .replace(", USES_CLASS]", "]")
            .replace("edge_types: [USES_CLASS]", "edge_types: [IMPORTS]"),
    )
    .unwrap();
    let styles = fixture.run(&["styles"], "clean");
    assert_eq!(styles.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&styles.stderr).contains("needs `provider.css: true`"));
}

const HTTP_YAML: &str = r#"version: 1
project: {name: http-fixture, root: app, source_roots: [src]}
provider: {kind: gitnexus, command: intentionally-missing-fallback, edge_types: [IMPORTS, FETCHES], http: true}
policies: {unassigned_files: ignore}
nodes:
  app: {}
  app.client: {maps: ["src/web/**"]}
  app.ui: {maps: ["src/ui/**"]}
  app.api: {maps: ["src/api/**"]}
rules:
  - {id: ui-calls-through-the-client, kind: deny_dependency, from: app.ui, to: app.api, edge_types: [FETCHES]}
"#;

#[test]
fn http_calls_reach_routes_as_fetches_edges_and_the_http_report() {
    let fixture = Fixture::new();
    let root = fixture.root.path();
    std::fs::write(root.join("architecture.yaml"), HTTP_YAML).unwrap();
    for directory in ["src/web", "src/ui", "src/api"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(root.join("src/api/app.py"), "").unwrap();
    std::fs::write(
        root.join("src/web/client.ts"),
        "const BASE = '/api'\n\
         function request(path: string, init?: RequestInit) {\n  return fetch(BASE + path, { ...init })\n}\n\
         export const list = () => request('/items')\n\
         export const add = () => request('/items', { method: 'POST' })\n\
         export const gone = () => request('/missing')\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/ui/view.tsx"),
        "export const load = (url: string) => fetch('/api/items', { method: 'DELETE' })\n\
         export const reload = () => fetch('/api/items')\n",
    )
    .unwrap();

    let check = fixture.run(&["check"], "routes");
    let text = String::from_utf8_lossy(&check.stdout);
    assert_eq!(check.status.code(), Some(2), "{text}");
    assert!(
        text.contains("src/ui/view.tsx -> src/api/app.py  (http-call, confidence 1)"),
        "{text}"
    );
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains("2 HTTP call(s) reach no route or use a method the route does not accept"),
        "{stderr}"
    );

    let report = fixture.run(&["http"], "routes");
    let report = String::from_utf8_lossy(&report.stdout);
    for expected in [
        "Routes: 3 (2 called by client code)",
        "  GET    /api/items    src/ui/view.tsx:2, src/web/client.ts:5",
        "  POST   /api/items    src/web/client.ts:6",
        "  GET    /{path:path}  not called",
        "  GET    /api/missing  src/web/client.ts:7",
        "  DELETE /api/items  src/ui/view.tsx:1  (the route does not accept this method)",
    ] {
        assert!(
            report.contains(expected),
            "missing {expected:?} in\n{report}"
        );
    }
    let scoped: Value =
        serde_json::from_slice(&fixture.run(&["http", "app.ui", "--json"], "routes").stdout)
            .unwrap();
    assert_eq!(scoped["routes"].as_array().unwrap().len(), 1);
    assert_eq!(scoped["unmatched"][0]["problem"], "wrong_method");

    // Without FETCHES in edge_types, provider.http could observe nothing.
    std::fs::write(
        root.join("architecture.yaml"),
        HTTP_YAML
            .replace("[IMPORTS, FETCHES]", "[IMPORTS]")
            .replace("edge_types: [FETCHES]", "edge_types: [IMPORTS]"),
    )
    .unwrap();
    let invalid = fixture.run(&["check"], "routes");
    assert_eq!(invalid.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("does not list FETCHES"));
    std::fs::write(
        root.join("architecture.yaml"),
        HTTP_YAML
            .replace(", http: true", "")
            .replace("edge_types: [FETCHES]", "edge_types: [IMPORTS]"),
    )
    .unwrap();
    let http = fixture.run(&["http"], "routes");
    assert_eq!(http.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&http.stderr).contains("needs `provider.http: true`"));
}

const PACKAGES_YAML: &str = r#"version: 1
project: {name: packages-fixture, root: app, source_roots: [src]}
provider: {kind: gitnexus, command: intentionally-missing-fallback, packages: true}
policies: {unassigned_files: ignore}
nodes:
  app: {}
  app.web: {maps: ["src/web/**"]}
  app.py: {maps: ["src/py/**"]}
  libs: {kind: external, title: Libraries}
  libs.maps: {kind: external, title: Leaflet, maps: ["package:npm/leaflet"]}
rules:
  - {id: python-draws-no-maps, kind: deny_dependency, from: app.py, to: libs.maps}
  - {id: web-draws-maps-elsewhere, kind: deny_dependency, from: app.web, to: libs.maps}
"#;

#[test]
fn imported_packages_become_edges_rules_and_the_packages_report() {
    let fixture = Fixture::new();
    let root = fixture.root.path();
    std::fs::write(root.join("architecture.yaml"), PACKAGES_YAML).unwrap();
    for directory in ["src/web/utils", "src/py"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(
        root.join("src/web/package.json"),
        r#"{"name": "web", "dependencies": {"leaflet": "1", "react": "19"}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("src/web/utils/date.ts"),
        "export const date = 1\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/web/app.tsx"),
        "import { useState } from 'react'\n\
         import L from 'leaflet'\n\
         import { date } from '@app/utils/date'\n\
         import { settings } from 'config'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/py/main.py"),
        "import numpy as np\nimport json\n",
    )
    .unwrap();

    let check = fixture.run(&["check"], "packages");
    let text = String::from_utf8_lossy(&check.stdout);
    assert_eq!(check.status.code(), Some(2), "{text}");
    assert!(text.contains("[web-draws-maps-elsewhere]"), "{text}");
    assert!(
        text.contains("src/web/app.tsx -> package:npm/leaflet  (package-import, confidence 1)"),
        "{text}"
    );
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains("1 import(s) may name a repository module or a package"),
        "{stderr}"
    );

    let report = fixture.run(&["packages"], "packages");
    let report = String::from_utf8_lossy(&report.stdout);
    for expected in [
        "Packages: 3 imported by 2 file(s)",
        "leaflet  (npm, package:npm/leaflet; node libs.maps)",
        "  imported by 1 file(s) in app.web",
        "  src/web/app.tsx:2  leaflet  app.web",
        "numpy  (Python, package:python/numpy; node packages)",
        "  src/py/main.py:1  numpy  app.py",
        "Imports that may name a repository module or a package (1); not observed:",
        "  src/web/app.tsx:4  config",
    ] {
        assert!(
            report.contains(expected),
            "missing {expected:?} in\n{report}"
        );
    }
    assert!(
        !report.contains("@app/utils"),
        "the resolved alias is local:\n{report}"
    );
    let one: Value = serde_json::from_slice(
        &fixture
            .run(&["packages", "react", "--json"], "packages")
            .stdout,
    )
    .unwrap();
    assert_eq!(one["packages"].as_array().unwrap().len(), 1);
    assert_eq!(one["packages"][0]["imports"][0]["line"], 1);
    let missing = fixture.run(&["packages", "left-pad"], "packages");
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr)
        .contains("no imported package is named `left-pad`"));
    let show = fixture.run(&["show", "packages"], "packages");
    let show = String::from_utf8_lossy(&show.stdout);
    assert!(show.contains("numpy — Python package"), "{show}");
    assert!(
        show.contains("app.py -> package:python/numpy  IMPORTS × 1"),
        "{show}"
    );

    // A package glob without `packages: true` could never match.
    std::fs::write(
        root.join("architecture.yaml"),
        PACKAGES_YAML.replace(", packages: true", ""),
    )
    .unwrap();
    let invalid = fixture.run(&["check"], "packages");
    assert_eq!(invalid.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&invalid.stderr)
        .contains("only `provider.packages: true` observes"));
    std::fs::write(
        root.join("architecture.yaml"),
        PACKAGES_YAML
            .replace(", packages: true", "")
            .replace(", maps: [\"package:npm/leaflet\"]", ""),
    )
    .unwrap();
    let packages = fixture.run(&["packages"], "packages");
    assert_eq!(packages.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&packages.stderr).contains("needs `provider.packages: true`"));
}

#[test]
fn the_work_directory_keeps_itself_out_of_git() {
    let fixture = Fixture::new();
    let root = fixture.root.path();
    let git_status = || {
        let output = Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=all"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    // An index fingerprint, so that the provider cache is written too.
    std::fs::create_dir(root.join(".gitnexus")).unwrap();
    std::fs::write(root.join(".gitnexus/meta.json"), "{}").unwrap();
    assert_eq!(fixture.run(&["check"], "clean").status.code(), Some(0));
    assert!(root.join(".archgraph/cache/provider.json").is_file());
    let ignore = root.join(".archgraph/.gitignore");
    assert_eq!(std::fs::read_to_string(&ignore).unwrap(), "*\n");
    let status = git_status();
    assert!(!status.contains(".archgraph"), "{status}");
    // A .gitignore someone changed is left as it is.
    std::fs::write(&ignore, "cache/\n").unwrap();
    assert_eq!(
        fixture.run(&["check", "--no-cache"], "clean").status.code(),
        Some(0)
    );
    assert_eq!(std::fs::read_to_string(&ignore).unwrap(), "cache/\n");
}

#[test]
fn a_file_moved_since_the_baseline_keeps_its_accepted_observations() {
    let fixture = Fixture::new();
    let root = fixture.root.path();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    // Git does not match empty files as renames.
    let source: String = (0..30).map(|line| format!("fn f{line}() {{}}\n")).collect();
    std::fs::write(root.join("src/a.rs"), &source).unwrap();
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "base"]);
    assert!(fixture.run(&["baseline"], "violation").status.success());
    let baseline_path = root.join("architecture.baseline.json");
    let baseline: Value =
        serde_json::from_str(&std::fs::read_to_string(&baseline_path).unwrap()).unwrap();
    assert_eq!(baseline["commit"].as_str().unwrap().len(), 40);

    // Moved and edited in the working tree, not committed.
    std::fs::remove_file(root.join("src/a.rs")).unwrap();
    std::fs::write(root.join("src/a_renamed.rs"), source + "fn extra() {}\n").unwrap();
    let output = fixture.run(&["check"], "violation_renamed");
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(
        text.contains("1 accepted observation(s) follow files renamed since the baseline"),
        "{text}"
    );

    // Without the commit there is nothing to follow: the move looks new.
    let mut without_commit = baseline.clone();
    without_commit.as_object_mut().unwrap().remove("commit");
    std::fs::write(&baseline_path, without_commit.to_string()).unwrap();
    assert_eq!(
        fixture.run(&["check"], "violation_renamed").status.code(),
        Some(2)
    );
}

fn http_get(port: u16, path: &str) -> Option<Value> {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let body = response.split_once("\r\n\r\n")?.1;
    serde_json::from_str(body).ok()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Polls until `condition` holds for `/api/meta`, or panics after 20 s.
fn wait_for_meta(port: u16, what: &str, condition: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..200 {
        if let Some(meta) = http_get(port, "/api/meta").filter(|meta| condition(meta)) {
            return meta;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn serve_reloads_when_the_configuration_or_the_index_changes() {
    let fixture = Fixture::new();
    let root = fixture.root.path();
    std::fs::create_dir(root.join(".gitnexus")).unwrap();
    std::fs::write(root.join(".gitnexus/meta.json"), "{}").unwrap();
    let port = free_port();
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _server = Kill(
        Command::new(env!("CARGO_BIN_EXE_archgraph"))
            .arg("--root")
            .arg(root)
            .args([
                "serve",
                "--refresh-seconds",
                "1",
                "--port",
                &port.to_string(),
            ])
            .env("GITNEXUS_BIN", &fixture.executable)
            .env("ARCHGRAPH_FAKE_MODE", "violation")
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );

    let meta = wait_for_meta(port, "the server", |meta| meta["revision"] == 1);
    assert_eq!(meta["watching"], true);
    let violations = http_get(port, "/api/violations").unwrap();
    assert_eq!(violations["violations"].as_array().unwrap().len(), 1);

    // A configuration edit: the rule now targets a node with no dependency.
    std::fs::write(
        root.join("architecture.yaml"),
        YAML.replace("to: app.b}", "to: app.c}"),
    )
    .unwrap();
    wait_for_meta(port, "a reload after the config edit", |meta| {
        meta["revision"] == 2
    });
    let violations = http_get(port, "/api/violations").unwrap();
    assert!(violations["violations"].as_array().unwrap().is_empty());

    // A broken configuration keeps the last good revision and says why.
    std::fs::write(root.join("architecture.yaml"), "version: [").unwrap();
    let meta = wait_for_meta(port, "a reported reload failure", |meta| {
        !meta["refresh_error"].is_null()
    });
    assert_eq!(meta["revision"], 2);
    std::fs::write(root.join("architecture.yaml"), YAML).unwrap();
    wait_for_meta(port, "a reload after the fix", |meta| meta["revision"] == 3);

    // A reindex (the index files change) reloads as well.
    std::fs::write(root.join(".gitnexus/meta.json"), "{\"reindexed\": true}").unwrap();
    let meta = wait_for_meta(port, "a reload after reindexing", |meta| {
        meta["revision"] == 4
    });
    assert!(meta["refresh_error"].is_null());
}
