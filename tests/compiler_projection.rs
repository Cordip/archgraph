use archgraph::{
    cli, compiler, config, context,
    model::{ArchitectureIr, CodeEdge, EdgeOrigin, FileUsage},
    projection::{self, EntryKind},
    provider::InMemoryProvider,
    server,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use std::{path::Path, sync::Arc};
use tower::ServiceExt;

const CONFIG: &str = include_str!("fixtures/architecture.yaml");
fn write(root: &Path, relative: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "fixture source content, never parsed by ArchGraph\n").unwrap();
}
fn repository() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    for path in [
        "src/api/routes.py",
        "src/api/handlers/route.py",
        "src/domain/model.py",
        "src/persistence/db.py",
        "src/shared/types.py",
        "src/main.py",
    ] {
        write(temp.path(), path);
    }
    temp
}
fn edge(from: &str, to: &str) -> CodeEdge {
    CodeEdge {
        from_file: from.into(),
        to_file: to.into(),
        kind: "IMPORTS".into(),
        confidence: Some(0.8),
        reason: Some("fixture".into()),
    }
}
fn imports() -> Vec<CodeEdge> {
    vec![
        edge("src/api/routes.py", "src/domain/model.py"),
        edge("src/api/handlers/route.py", "src/domain/model.py"),
        edge("src/domain/model.py", "src/persistence/db.py"),
        edge("src/persistence/db.py", "src/domain/model.py"),
    ]
}
async fn compile(root: &Path, edges: Vec<CodeEdge>) -> ArchitectureIr {
    compiler::compile(
        root,
        &config::parse(CONFIG).unwrap(),
        &InMemoryProvider {
            edges,
            failure: None,
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn compiler_is_provider_agnostic_and_ir_roundtrips() {
    let temp = repository();
    let ir = compile(temp.path(), imports()).await;
    assert_eq!(ir.stats.mapped_file_count, 6);
    assert_eq!(ir.stats.violation_count, 3);
    assert_eq!(ir.nodes["app"].descendant_file_count, 6);
    assert_eq!(ir.nodes["app.api"].descendant_file_count, 2);
    assert_eq!(ir.nodes["app.api"].direct_files, ["src/api/routes.py"]);
    let bytes = serde_json::to_vec(&ir).unwrap();
    let roundtrip: ArchitectureIr = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&roundtrip).unwrap(), bytes);
    let path = compiler::persist(temp.path(), &ir).unwrap();
    let saved: ArchitectureIr = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved.stats.mapped_file_count, 6);
    assert!(path.ends_with(".archgraph/architecture.ir.json"));
}

#[tokio::test]
async fn deterministic_under_shuffled_provider_edges_and_repeated_compilation() {
    let temp = repository();
    let first = compile(temp.path(), imports()).await;
    compiler::persist(temp.path(), &first).unwrap();
    let mut reversed = imports();
    reversed.reverse();
    let second = compile(temp.path(), reversed).await;
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
}

#[tokio::test]
async fn an_index_rewritten_during_compilation_is_rejected() {
    let temp = repository();
    let provider = InMemoryProvider {
        edges: vec![edge("src/api/routes.py", "src/domain/model.py")],
        fingerprints: std::sync::Arc::new(std::sync::Mutex::new(vec![
            "before".into(),
            "after".into(),
        ])),
        ..Default::default()
    };
    let error = compiler::compile(
        temp.path(),
        &config::parse(CONFIG).unwrap(),
        &provider,
        None,
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("index changed while compiling"),
        "{error}"
    );
    let stable = InMemoryProvider {
        fingerprints: std::sync::Arc::new(std::sync::Mutex::new(vec!["same".into()])),
        ..Default::default()
    };
    assert!(
        compiler::compile(temp.path(), &config::parse(CONFIG).unwrap(), &stable, None)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn cached_provider_results_give_the_same_ir_until_the_index_changes() {
    let temp = repository();
    let provider = InMemoryProvider {
        edges: imports(),
        fingerprints: std::sync::Arc::new(std::sync::Mutex::new(vec!["index-1".into()])),
        ..Default::default()
    };
    let validated = config::parse(CONFIG).unwrap();
    let options = compiler::CompileOptions {
        reindex: None,
        cache: Some(temp.path().join(".archgraph/cache/provider.json")),
    };
    let run = || compiler::compile_with(temp.path(), &validated, &provider, &options);
    let queries = || provider.queries.load(std::sync::atomic::Ordering::Relaxed);
    let fresh = run().await.unwrap();
    let cached = run().await.unwrap();
    assert!(!fresh.cached && cached.cached);
    assert_eq!(queries(), 1);
    assert_eq!(
        serde_json::to_value(&fresh.ir).unwrap(),
        serde_json::to_value(&cached.ir).unwrap()
    );
    *provider.fingerprints.lock().unwrap() = vec!["index-2".into()];
    assert!(!run().await.unwrap().cached);
    assert_eq!(queries(), 2);
    // A provider that cannot identify its index is never cached.
    provider.fingerprints.lock().unwrap().clear();
    assert!(!run().await.unwrap().cached);
    assert!(!run().await.unwrap().cached);
    assert_eq!(queries(), 4);
}

#[tokio::test]
async fn provider_failure_is_not_a_clean_graph() {
    let temp = repository();
    let provider = InMemoryProvider {
        edges: Vec::new(),
        failure: Some("index is unavailable".into()),
        ..Default::default()
    };
    let result = compiler::compile(
        temp.path(),
        &config::parse(CONFIG).unwrap(),
        &provider,
        None,
    )
    .await;
    assert!(result.is_err());
    assert!(!temp.path().join(".archgraph/architecture.ir.json").exists());
}

#[tokio::test]
async fn unknown_and_unassigned_provider_paths_are_diagnostics() {
    let temp = repository();
    let ir = compile(
        temp.path(),
        vec![
            edge("outside.py", "src/domain/model.py"),
            edge("../escape.py", "src/domain/model.py"),
            edge("src/ghost.py", "src/domain/model.py"),
            // A symbol GitNexus stored without a file path.
            edge("src/api/routes.py", ""),
        ],
    )
    .await;
    assert_eq!(ir.stats.observed_edge_count, 4);
    assert_eq!(ir.stats.resolved_edge_count, 0);
    // outside.py is outside source_roots: expected, counted, not an anomaly.
    assert_eq!(ir.stats.out_of_scope_edge_count, 1);
    // An escaping path, an in-scope file that was never discovered and an
    // empty path are.
    assert_eq!(ir.diagnostics.provider_anomalies.len(), 3);
    assert!(ir
        .diagnostics
        .provider_anomalies
        .iter()
        .any(|a| a.to_file.is_empty() && a.message.contains("empty file path")));
    assert!(ir
        .diagnostics
        .warnings
        .iter()
        .any(|w| w.starts_with("1 provider edge(s) involve symbols stored without a file path")));
    assert!(ir
        .diagnostics
        .provider_anomalies
        .iter()
        .any(|a| a.from_file == "src/ghost.py" && a.message.contains("not discovered")));
    assert!(!ir.diagnostics.warnings.is_empty());
}

#[tokio::test]
async fn focus_projects_deep_edges_and_retains_direct_ownership() {
    let temp = repository();
    let ir = compile(temp.path(), imports()).await;
    let view = projection::project(&ir, "app", 20).unwrap();
    assert!(view.nodes.iter().any(|n| n.id == "node:app.api"));
    assert!(!view.nodes.iter().any(|n| n.id == "node:app.api.handlers"));
    assert!(view
        .nodes
        .iter()
        .any(|n| n.entry_kind == EntryKind::DirectFiles));
    let aggregated = view
        .edges
        .iter()
        .find(|e| e.edge.from == "node:app.api" && e.edge.to == "node:app.domain")
        .unwrap();
    assert_eq!(aggregated.edge.count, 2);
    assert_eq!(aggregated.edge.evidence.len(), 2);
    assert!(view.edges.iter().all(|e| e.edge.from != e.edge.to));
}

#[tokio::test]
async fn cross_boundary_incoming_outgoing_and_manual_edges_survive() {
    let temp = repository();
    let ir = compile(temp.path(), imports()).await;
    let view = projection::project(&ir, "app.domain", 20).unwrap();
    assert!(view.nodes.iter().any(|n| n.entry_kind == EntryKind::File
        && n.file_path.as_deref() == Some("src/domain/model.py")));
    assert!(view
        .nodes
        .iter()
        .any(|n| n.outside_focus && n.architecture_id.as_deref() == Some("app.persistence")));
    assert!(view
        .edges
        .iter()
        .any(|e| e.edge.from == "file:src/domain/model.py" && e.edge.to == "node:app.persistence"));
    assert!(view
        .edges
        .iter()
        .any(|e| e.edge.from == "node:app.persistence" && e.edge.to == "file:src/domain/model.py"));
    let root = projection::project(&ir, "app", 20).unwrap();
    let manual = root
        .edges
        .iter()
        .find(|e| e.edge.from == "boundary:app")
        .unwrap();
    assert_eq!(manual.edge.origin, EdgeOrigin::Manual);
    assert!(manual.edge.evidence.is_empty());
    assert!(root
        .nodes
        .iter()
        .any(|n| n.outside_focus && n.architecture_id.as_deref() == Some("external.service")));
}

#[tokio::test]
async fn leaf_projection_is_lossless_beyond_aggregated_evidence_cap() {
    let temp = repository();
    let mut edges = Vec::new();
    for i in 0..65 {
        let path = format!("src/domain/file{i:03}.py");
        write(temp.path(), &path);
        edges.push(edge(&path, "src/domain/model.py"));
    }
    let ir = compile(temp.path(), edges).await;
    let aggregated = ir
        .edges
        .iter()
        .find(|e| e.origin == EdgeOrigin::Observed)
        .unwrap();
    assert_eq!(aggregated.count, 65);
    assert_eq!(aggregated.evidence.len(), 20);
    let leaf = projection::project(&ir, "app.domain", 20).unwrap();
    assert_eq!(leaf.edges.len(), 65);
    assert_eq!(leaf.nodes.len(), 66);
}

#[tokio::test]
async fn context_increases_edge_and_violation_evidence_limit_from_raw_imports() {
    let temp = repository();
    let mut edges = Vec::new();
    for i in (0..65).rev() {
        let path = format!("src/domain/file{i:03}.py");
        write(temp.path(), &path);
        edges.push(edge(&path, "src/persistence/db.py"));
    }
    let ir = compile(temp.path(), edges).await;
    assert!(ir.violations.iter().all(|v| v.evidence.len() == 20));
    let result = context::build(&ir, "app", 50).unwrap();
    let edge = result
        .observed_internal
        .iter()
        .find(|e| e.edge.from == "node:app.domain")
        .unwrap();
    assert_eq!(edge.edge.count, 65);
    assert_eq!(edge.edge.evidence.len(), 50);
    assert!(result
        .projection
        .violations
        .iter()
        .all(|v| v.evidence.len() == 50));
    let markdown = context::markdown(&result);
    assert!(markdown.contains("Agent contract"));
    assert!(markdown.contains("gitnexus analyze --index-only"));
    let value = serde_json::to_value(&result).unwrap();
    assert!(value["agent_contract"].is_array());
}

#[tokio::test]
async fn manual_edges_do_not_trigger_rules_even_with_imports_kind() {
    let temp = repository();
    let mut validated = config::parse(CONFIG).unwrap();
    validated.config.edges[0].from = "app.domain".into();
    validated.config.edges[0].to = "app.persistence".into();
    validated.config.edges[0].kind = "IMPORTS".into();
    let ir = compiler::compile(temp.path(), &validated, &InMemoryProvider::default(), None)
        .await
        .unwrap();
    assert!(ir.violations.is_empty());
}

#[tokio::test]
async fn scoped_check_matches_actual_owners_not_similarly_named_or_unaffected_descendants() {
    let temp = repository();
    let yaml = CONFIG.replace(
        "from: app.domain\n    to: app.persistence",
        "from: app.api\n    to: app.domain\n    include_descendants: false",
    );
    let provider = InMemoryProvider {
        edges: vec![
            edge("src/api/routes.py", "src/domain/model.py"),
            edge("src/api/handlers/route.py", "src/domain/model.py"),
        ],
        failure: None,
        ..Default::default()
    };
    let ir = compiler::compile(temp.path(), &config::parse(&yaml).unwrap(), &provider, None)
        .await
        .unwrap();
    assert_eq!(
        cli::matching_violations(&ir, Some("app.api"))
            .unwrap()
            .len(),
        1
    );
    assert!(cli::matching_violations(&ir, Some("app.api.handlers"))
        .unwrap()
        .is_empty());
    assert!(cli::matching_violations(&ir, Some("missing")).is_err());
    let view = projection::project(&ir, "app.api", 20).unwrap();
    let child_edge = view
        .edges
        .iter()
        .find(|e| e.edge.from == "node:app.api.handlers")
        .unwrap();
    assert!(child_edge.violation_rule_ids.is_empty());
}

#[tokio::test]
async fn a_live_server_serves_each_published_revision_and_reports_failed_reloads() {
    let temp = repository();
    let before = compile(temp.path(), imports()).await;
    let after = compile(temp.path(), imports()[..1].to_vec()).await;
    let (before_count, after_count) = (before.violations.len(), after.violations.len());
    assert_ne!(before_count, after_count);
    let live = server::Live::new(before, true);
    let app = server::live_router(live.clone());
    let get = |path: &'static str| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        }
    };
    let meta = get("/api/meta").await;
    assert_eq!(
        (meta["watching"].as_bool(), meta["revision"].as_u64()),
        (Some(true), Some(1))
    );
    let violation_count = || async {
        get("/api/violations").await["violations"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(violation_count().await, before_count);

    assert_eq!(live.publish(after), 2);
    assert_eq!(get("/api/meta").await["revision"], 2);
    assert_eq!(violation_count().await, after_count);

    live.fail("index changed while compiling".into());
    let meta = get("/api/meta").await;
    assert_eq!(
        meta["revision"], 2,
        "a failed reload keeps the last good revision"
    );
    assert_eq!(meta["refresh_error"], "index changed while compiling");
}

#[tokio::test]
async fn read_only_http_endpoints_share_projection_and_do_not_expose_source_or_cypher() {
    let temp = repository();
    let ir = compile(temp.path(), imports()).await;
    let expected =
        serde_json::to_value(projection::project(&ir, "app.domain", 20).unwrap()).unwrap();
    let app = server::router(Arc::new(ir));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/focus/app.domain")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key("content-security-policy"));
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let actual: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(actual, expected);
    for path in ["/api/focus/missing", "/api/cypher", "/src/domain/model.py"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/meta")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/search?q=domain")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value[0]["id"], "app.domain");
}

#[test]
fn distributed_example_and_fixture_obey_schema() {
    assert!(config::parse(CONFIG).is_ok());
    assert!(config::parse(include_str!("../architecture.example.yaml")).is_ok());
    for example in [
        include_str!("../examples/zammad/architecture.yaml"),
        include_str!("../examples/lct-task3/architecture.yaml"),
    ] {
        config::parse(example).unwrap();
    }
}

#[tokio::test]
async fn layers_put_dependents_above_their_dependencies() {
    let temp = repository();
    let ir = compile(
        temp.path(),
        vec![
            edge("src/api/routes.py", "src/domain/model.py"),
            edge("src/api/handlers/route.py", "src/domain/model.py"),
            edge("src/domain/model.py", "src/persistence/db.py"),
            // One upward dependency: domain stays above persistence anyway.
            edge("src/persistence/db.py", "src/domain/model.py"),
            edge("src/domain/model.py", "src/shared/types.py"),
        ],
    )
    .await;
    let view = projection::project(&ir, "app", 20).unwrap();
    let row_of = |id: &str| {
        view.layers
            .iter()
            .position(|row| row.iter().any(|entry| entry == id))
    };
    assert!(
        row_of("node:app.api") < row_of("node:app.domain"),
        "{:?}",
        view.layers
    );
    assert!(
        row_of("node:app.domain") < row_of("node:app.persistence"),
        "{:?}",
        view.layers
    );
    let inside = view.nodes.iter().filter(|node| !node.outside_focus).count();
    assert_eq!(view.layers.iter().map(Vec::len).sum::<usize>(), inside);
}

#[tokio::test]
async fn files_are_entry_points_used_unused_or_unknown_and_nodes_count_them() {
    let temp = repository();
    write(temp.path(), "src/shared/legacy.py");
    let config = CONFIG.replace(
        "source_roots: [src]",
        "source_roots: [src]\n  entry_points: [src/main.py, \"src/missing/*.py\"]",
    );
    let indexed: Vec<String> = [
        "src/main.py",
        "src/api/routes.py",
        "src/domain/model.py",
        "src/shared/types.py",
    ]
    .map(String::from)
    .to_vec();
    let provider = InMemoryProvider {
        edges: vec![
            edge("src/main.py", "src/api/routes.py"),
            edge("src/api/routes.py", "src/domain/model.py"),
            // Its only user is a script outside source_roots.
            edge("scripts/run.py", "src/api/handlers/route.py"),
            // Not in the index, but observed users beat "unknown".
            edge("src/domain/model.py", "src/persistence/db.py"),
            // A file's dependency on itself is no user.
            edge("src/shared/types.py", "src/shared/types.py"),
        ],
        indexed: Some(indexed),
        ..Default::default()
    };
    let ir = compiler::compile(
        temp.path(),
        &config::parse(&config).unwrap(),
        &provider,
        None,
    )
    .await
    .unwrap();
    let usage: Vec<(&str, Option<FileUsage>)> = ir
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.usage))
        .collect();
    assert_eq!(
        usage,
        [
            ("src/api/handlers/route.py", Some(FileUsage::Used)),
            ("src/api/routes.py", Some(FileUsage::Used)),
            ("src/domain/model.py", Some(FileUsage::Used)),
            ("src/main.py", Some(FileUsage::EntryPoint)),
            ("src/persistence/db.py", Some(FileUsage::Used)),
            ("src/shared/legacy.py", Some(FileUsage::NotIndexed)),
            ("src/shared/types.py", Some(FileUsage::NoObservedUsers)),
        ]
    );
    assert!(
        ir.diagnostics
            .warnings
            .iter()
            .any(|warning| warning.contains("`src/missing/*.py` matches no mapped file")),
        "{:?}",
        ir.diagnostics.warnings
    );
    let counts = |id: &str| {
        let node = &ir.nodes[id];
        (
            node.entry_point_count,
            node.no_observed_users_count,
            node.outside_user_count,
            node.no_outside_users(),
        )
    };
    assert_eq!(counts("app"), (1, 1, 1, false));
    // main.py (in `app` itself) and the script use the API from outside.
    assert_eq!(counts("app.api"), (0, 0, 2, false));
    assert_eq!(counts("app.api.handlers"), (0, 0, 1, false));
    assert_eq!(counts("app.persistence"), (0, 0, 1, false));
    // Nothing outside uses it: the whole node is a candidate.
    assert_eq!(counts("app.shared"), (0, 1, 0, true));

    let root = projection::project(&ir, "app", 20).unwrap();
    let shared = root
        .nodes
        .iter()
        .find(|node| node.id == "node:app.shared")
        .unwrap();
    assert_eq!(shared.no_observed_users_count, 1);
    assert!(shared.no_outside_users);
    let direct = root
        .nodes
        .iter()
        .find(|node| node.entry_kind == EntryKind::DirectFiles)
        .unwrap();
    assert_eq!(direct.entry_point_count, 1);
    let leaf = projection::project(&ir, "app.shared", 20).unwrap();
    let file = |path: &str| {
        leaf.nodes
            .iter()
            .find(|node| node.file_path.as_deref() == Some(path))
            .unwrap()
            .usage
    };
    assert_eq!(
        file("src/shared/types.py"),
        Some(FileUsage::NoObservedUsers)
    );
    assert_eq!(file("src/shared/legacy.py"), Some(FileUsage::NotIndexed));

    // Without declared entry points the IR's project has no trace of them.
    let plain = compile(temp.path(), imports()).await;
    assert!(!serde_json::to_string(&plain.project)
        .unwrap()
        .contains("entry_points"));
}

const PACKAGES_CONFIG: &str = r#"version: 1
project: {name: packages, root: app, source_roots: [src]}
provider: {kind: gitnexus, packages: true}
nodes:
  app: {maps: ["src/**"]}
  app.api: {maps: ["src/api/**"]}
  app.core: {maps: ["src/core/**"]}
  libs: {kind: external, title: Libraries}
  libs.solver: {kind: external, title: OR-Tools, maps: ["package:python/ortools"]}
rules:
  - {id: only-core-solves, kind: deny_dependency, from: app.api, to: libs.solver}
"#;

fn package_repository() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    for (path, source) in [
        (
            "src/api/routes.py",
            "import fastapi\nfrom ortools.sat.python import cp_model\nfrom core import plan\n",
        ),
        (
            "src/core/plan.py",
            "from ortools.constraint_solver import pywrapcp\nimport dataclasses\n",
        ),
        ("src/core/__init__.py", ""),
    ] {
        let path = temp.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    temp
}

#[tokio::test]
async fn imported_packages_are_mapped_ruled_projected_and_kept_out_of_coverage() {
    let temp = package_repository();
    let validated = config::parse(PACKAGES_CONFIG).unwrap();
    let provider = InMemoryProvider {
        edges: vec![edge("src/api/routes.py", "src/core/plan.py")],
        ..Default::default()
    };
    let ir = compiler::compile(temp.path(), &validated, &provider, None)
        .await
        .unwrap();
    let report = ir.packages.as_ref().unwrap();
    let owners: Vec<(&str, Option<&str>)> = report
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package.node.as_deref()))
        .collect();
    assert_eq!(
        owners,
        [
            ("package:python/fastapi", Some("packages")),
            ("package:python/ortools", Some("libs.solver")),
        ]
    );
    assert_eq!(ir.nodes["packages"].packages, ["package:python/fastapi"]);
    assert_eq!(ir.nodes["libs"].descendant_package_count, 1);
    assert_eq!(ir.stats.package_row_count, 3);
    // No file counts a package, and an import of one is no coverage.
    assert_eq!(ir.nodes["app"].descendant_file_count, 3);
    assert_eq!(ir.nodes["app.api"].observed_file_count, 1);
    assert_eq!(ir.stats.unassigned_file_count, 0);
    assert_eq!(ir.violations.len(), 1, "{:#?}", ir.violations);
    let violation = &ir.violations[0];
    assert_eq!(violation.to.as_deref(), Some("libs.solver"));
    assert_eq!(violation.evidence[0].from_file, "src/api/routes.py");
    assert_eq!(violation.evidence[0].to_file, "package:python/ortools");
    assert_eq!(
        violation.evidence[0].reason.as_deref(),
        Some("package-import")
    );

    // Outside the root, the owners appear like any external node.
    let root = projection::project(&ir, "app", 20).unwrap();
    let outside: Vec<(&str, usize)> = root
        .nodes
        .iter()
        .filter(|node| node.outside_focus)
        .map(|node| (node.id.as_str(), node.package_count))
        .collect();
    assert_eq!(outside, [("node:libs.solver", 1), ("node:packages", 1)]);
    // At its owner, each package is an entry that knows who imports it.
    let packages = projection::project(&ir, "packages", 20).unwrap();
    let fastapi = packages
        .nodes
        .iter()
        .find(|node| node.entry_kind == EntryKind::Package)
        .unwrap();
    assert_eq!(fastapi.id, "package:python/fastapi");
    assert_eq!(fastapi.title, "fastapi");
    let imports = &fastapi.package.as_ref().unwrap().imports;
    assert_eq!(
        (
            imports[0].file.as_str(),
            imports[0].line,
            imports[0].node.as_deref()
        ),
        ("src/api/routes.py", 1, Some("app.api"))
    );
    assert!(packages
        .edges
        .iter()
        .any(|edge| edge.edge.from == "node:app.api" && edge.edge.to == "package:python/fastapi"));
    let text = archgraph::render::text::render(&packages);
    assert!(
        text.contains("fastapi — Python package\n    package:python/fastapi"),
        "{text}"
    );
    // Importing a package makes no file and no package node "used".
    let usage = |path: &str| {
        ir.files
            .iter()
            .find(|file| file.path == path)
            .unwrap()
            .usage
    };
    assert_eq!(usage("src/core/plan.py"), Some(FileUsage::Used));
    assert_eq!(usage("src/api/routes.py"), Some(FileUsage::NoObservedUsers));
    assert_eq!(ir.nodes["libs.solver"].outside_user_count, 0);
    assert_eq!(ir.nodes["packages"].outside_user_count, 0);
    let context = context::build(&ir, "app.api", 20).unwrap();
    let markdown = context::markdown(&context);
    assert!(markdown.contains("- `ortools` (Python, `package:python/ortools`, node `libs.solver`): `src/api/routes.py:2`"), "{markdown}");

    let again = compiler::compile(temp.path(), &validated, &provider, None)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&ir).unwrap(),
        serde_json::to_vec(&again).unwrap()
    );
}

#[tokio::test]
async fn without_provider_packages_the_ir_has_no_trace_of_them() {
    let temp = package_repository();
    let config = PACKAGES_CONFIG
        .replace(", packages: true", "")
        .replace(", maps: [\"package:python/ortools\"]", "");
    let ir = compiler::compile(
        temp.path(),
        &config::parse(&config).unwrap(),
        &InMemoryProvider::default(),
        None,
    )
    .await
    .unwrap();
    let json = serde_json::to_string(&ir).unwrap();
    for absent in ["package:", "package_", "\"packages\":"] {
        assert!(!json.contains(absent), "{absent} in {json}");
    }
}

#[tokio::test]
async fn the_package_endpoint_lists_packages_for_search() {
    let temp = package_repository();
    let ir = compiler::compile(
        temp.path(),
        &config::parse(PACKAGES_CONFIG).unwrap(),
        &InMemoryProvider::default(),
        None,
    )
    .await
    .unwrap();
    let response = server::router(Arc::new(ir))
        .oneshot(
            Request::builder()
                .uri("/api/packages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["enabled"], true);
    assert_eq!(value["packages"][1]["name"], "ortools");
    assert_eq!(value["packages"][1]["node"], "libs.solver");
    assert_eq!(value["packages"][1]["file_count"], 2);
    assert_eq!(
        value["packages"][1]["nodes"],
        serde_json::json!(["app.api", "app.core"])
    );
    let imports = value["packages"][1]["imports"].as_array().unwrap();
    assert_eq!(imports.len(), 2);
    assert!(imports
        .iter()
        .all(|import| import["line"].as_u64().unwrap() > 0
            && import["file"].as_str().is_some()
            && import["specifier"].as_str().unwrap().starts_with("ortools")));
}

#[tokio::test]
async fn every_script_and_stylesheet_the_page_loads_is_served() {
    let temp = repository();
    let app = server::router(Arc::new(compile(temp.path(), imports()).await));
    let page = include_str!("../src/web/index.html");
    let assets: Vec<&str> = page
        .split(['"', '\''])
        .filter(|part| part.starts_with('/') && (part.ends_with(".js") || part.ends_with(".css")))
        .collect();
    assert!(
        assets.contains(&"/board.js") && assets.contains(&"/app.js"),
        "{assets:?}"
    );
    for asset in assets {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(asset).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{asset}");
        let kind = response.headers()["content-type"]
            .to_str()
            .unwrap()
            .to_owned();
        let expected = if asset.ends_with(".js") {
            "text/javascript"
        } else {
            "text/css"
        };
        assert!(kind.starts_with(expected), "{asset}: {kind}");
        assert!(response.headers().contains_key("content-security-policy"));
    }
}
