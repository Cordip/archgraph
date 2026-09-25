//! Contract test against a real GitNexus installation. The fake provider in
//! `fixtures/fake_gitnexus.py` encodes our assumptions about GitNexus; this
//! checks them. It missed the 64 KiB stdout truncation and Markdown IMPORTS,
//! which only a real run revealed. Run explicitly:
//!
//!     cargo test --test gitnexus_contract -- --ignored
//!
//! Requires `gitnexus` on PATH (or GITNEXUS_BIN). HOME is redirected to a
//! temporary directory so the global GitNexus registry, and any auto-index
//! service watching it, never sees the fixture repository.
#![cfg(unix)]

use serde_json::Value;
use std::{path::Path, process::Command};

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

#[test]
#[ignore = "needs a real GitNexus installation; run with --ignored"]
fn archgraph_reads_a_real_gitnexus_index() {
    if Command::new(gitnexus()).arg("--version").output().is_err() {
        panic!("gitnexus is not installed; install it or set GITNEXUS_BIN");
    }
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
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

    let analyze = Command::new(gitnexus())
        .args(["analyze", "--index-only", "--skip-git"])
        .arg(&repo)
        .current_dir(&repo)
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(
        analyze.status.success(),
        "gitnexus analyze failed: {}",
        String::from_utf8_lossy(&analyze.stderr)
    );

    let check = Command::new(env!("CARGO_BIN_EXE_archgraph"))
        .args(["--root"])
        .arg(&repo)
        .args(["check", "--json"])
        .env("HOME", &home)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert_eq!(check.status.code(), Some(2), "{stderr}");
    let report: Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(report["violation_count"], 1, "{report:#}");
    let violation = &report["violations"][0];
    assert_eq!(violation["from"], "app.lib");
    assert_eq!(violation["to"], "app.features");

    let ir: Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".archgraph/architecture.ir.json")).unwrap(),
    )
    .unwrap();
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
}
