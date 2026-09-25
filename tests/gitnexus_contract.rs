//! Contract test against a real GitNexus installation. The fake provider in
//! `fixtures/fake_gitnexus.py` encodes our assumptions about GitNexus; this
//! checks them. It missed the 64 KiB stdout truncation and Markdown IMPORTS,
//! which only a real run revealed. Run explicitly:
//!
//!     cargo test --test gitnexus_contract -- --ignored
//!
//! Requires `gitnexus` on PATH (or GITNEXUS_BIN; on Windows the full path of
//! `gitnexus.cmd`). HOME (USERPROFILE on Windows) is redirected to a temporary
//! directory so the global GitNexus registry, and any auto-index service
//! watching it, never sees the fixture repositories.

use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Output},
};

const FEATURES: usize = 500;

const YAML: &str = r#"version: 1
project: {name: contract, root: app, source_roots: [src]}
provider: {kind: gitnexus, edge_types: [IMPORTS, CALLS]}
nodes:
  app: {}
  app.features: {maps: ["src/features/**"]}
  app.lib: {maps: ["src/lib/**"]}
rules:
  - {id: lib-does-not-use-features, kind: deny_dependency, from: app.lib, to: app.features, edge_types: [IMPORTS, CALLS]}
"#;

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn gitnexus() -> String {
    std::env::var("GITNEXUS_BIN").unwrap_or_else(|_| "gitnexus".into())
}

fn isolated(command: &mut Command, home: &Path) -> Output {
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .output()
        .unwrap()
}

/// Indexes `repo` with the real GitNexus under an isolated home directory.
fn analyze(repo: &Path, home: &Path) {
    if Command::new(gitnexus()).arg("--version").output().is_err() {
        panic!("gitnexus is not installed; install it or set GITNEXUS_BIN");
    }
    std::fs::create_dir_all(home).unwrap();
    let analyze = isolated(
        Command::new(gitnexus())
            .args(["analyze", "--index-only", "--skip-git"])
            .arg(repo)
            .current_dir(repo),
        home,
    );
    assert!(
        analyze.status.success(),
        "gitnexus analyze failed: {}",
        String::from_utf8_lossy(&analyze.stderr)
    );
}

fn archgraph(repo: &Path, home: &Path, args: &[&str]) -> Output {
    isolated(
        Command::new(env!("CARGO_BIN_EXE_archgraph"))
            .arg("--root")
            .arg(repo)
            .args(args),
        home,
    )
}

fn ir(repo: &Path) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(repo.join(".archgraph/architecture.ir.json")).unwrap(),
    )
    .unwrap()
}

#[test]
#[ignore = "needs a real GitNexus installation; run with --ignored"]
fn archgraph_reads_a_real_gitnexus_index() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    write(
        &repo,
        "src/lib/helper.ts",
        "export function helper(value: number): number {\n  return value + 1;\n}\n",
    );
    // Many long paths: one result page is far larger than a 64 KiB pipe buffer.
    for index in 0..FEATURES {
        write(
            &repo,
            &format!("src/features/feature_module_{index:03}/component_with_a_descriptive_long_name.ts"),
            &format!("import {{ helper }} from '../../lib/helper';\n\nexport const value{index} = helper({index});\n"),
        );
    }
    // The one violation: lib imports and calls a feature.
    write(
        &repo,
        "src/lib/bad.ts",
        "import { value0 } from '../features/feature_module_000/component_with_a_descriptive_long_name';\nimport { helper } from './helper';\n\nexport const bad = helper(value0);\n",
    );
    // GitNexus reports Markdown links as IMPORTS; they must be filtered out.
    write(&repo, "src/README.md", "See [the helper](lib/helper.ts).\n");
    write(&repo, "architecture.yaml", YAML);
    analyze(&repo, &home);

    let check = archgraph(&repo, &home, &["check", "--json"]);
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert_eq!(check.status.code(), Some(2), "{stderr}");
    let report: Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(report["violation_count"], 1, "{report:#}");
    let violation = &report["violations"][0];
    assert_eq!(violation["from"], "app.lib");
    assert_eq!(violation["to"], "app.features");

    let ir = ir(&repo);
    let stats = &ir["stats"];
    let kinds: Vec<&str> = ir["resolved_edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|edge| edge["kind"].as_str())
        .collect();
    assert!(
        stats["observed_edge_count"].as_u64().unwrap() > FEATURES as u64,
        "{stats:#}"
    );
    assert!(
        kinds.contains(&"IMPORTS") && kinds.contains(&"CALLS"),
        "relation kinds seen: {kinds:?}"
    );
    assert!(
        stats["filtered_edge_count"].as_u64().unwrap() >= 1,
        "Markdown link was not filtered: {stats:#}"
    );
    assert!(
        ir["resolved_edges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|edge| !edge["evidence"]["from_file"]
                .as_str()
                .unwrap()
                .ends_with(".md")),
        "a Markdown link survived as a dependency"
    );

    // Queries must leave the index fingerprint alone, or the cache never hits.
    let again = archgraph(&repo, &home, &["compile"]);
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("(cached: index unchanged)"),
        "{}",
        String::from_utf8_lossy(&again.stdout)
    );
    assert_eq!(ir, self::ir(&repo), "the cache changed the IR");
}

const RUBY_YAML: &str = r#"version: 1
project: {name: rails, root: app, source_roots: [app, lib]}
provider: {kind: gitnexus, edge_types: [IMPORTS, CALLS, EXTENDS, IMPLEMENTS], min_confidence: 0.6}
nodes:
  app: {}
  app.models: {maps: ["app/models/**"]}
  app.lib: {maps: ["lib/**"]}
rules:
  - {id: lib-does-not-know-models, kind: deny_dependency, from: app.lib, to: app.models, edge_types: [CALLS]}
"#;

/// Rails autoloads constants, so nothing is imported: the zammad example and
/// `init --suggest` rely on Ruby dependencies arriving as CALLS, EXTENDS
/// (superclass) and IMPLEMENTS (`include`).
#[test]
#[ignore = "needs a real GitNexus installation; run with --ignored"]
fn ruby_dependencies_arrive_as_calls_extends_and_implements() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    write(
        &repo,
        "app/models/application_record.rb",
        "class ApplicationRecord\n  def self.find(id)\n    new\n  end\nend\n",
    );
    write(
        &repo,
        "app/models/concerns/auditable.rb",
        "module Auditable\n  def audit\n    true\n  end\nend\n",
    );
    write(
        &repo,
        "app/models/user.rb",
        "class User < ApplicationRecord\n  include Auditable\n\n  def display_name\n    \"user\"\n  end\nend\n",
    );
    write(
        &repo,
        "lib/report.rb",
        "class Report\n  def run\n    User.find(1).display_name\n  end\nend\n",
    );
    write(&repo, "architecture.yaml", RUBY_YAML);
    analyze(&repo, &home);

    let check = archgraph(&repo, &home, &["check", "--json"]);
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert_eq!(check.status.code(), Some(2), "{stderr}");
    let report: Value = serde_json::from_slice(&check.stdout).unwrap();
    let violation = &report["violations"][0];
    assert_eq!(violation["edge_kind"], "CALLS", "{report:#}");
    assert_eq!(violation["evidence"][0]["from_file"], "lib/report.rb");

    let ir = ir(&repo);
    let edges: Vec<(&str, &str, &str)> = ir["resolved_edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| {
            (
                edge["evidence"]["from_file"].as_str().unwrap(),
                edge["evidence"]["to_file"].as_str().unwrap(),
                edge["kind"].as_str().unwrap(),
            )
        })
        .collect();
    for expected in [
        (
            "app/models/user.rb",
            "app/models/application_record.rb",
            "EXTENDS",
        ),
        (
            "app/models/user.rb",
            "app/models/concerns/auditable.rb",
            "IMPLEMENTS",
        ),
    ] {
        assert!(edges.contains(&expected), "{expected:?} missing: {edges:?}");
    }
}
