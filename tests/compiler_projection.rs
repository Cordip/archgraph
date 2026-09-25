use archgraph::{
    cli, compiler, config, context,
    model::{ArchitectureIr, CodeEdge, EdgeOrigin},
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
        },
        false,
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
async fn provider_failure_is_not_a_clean_graph() {
    let temp = repository();
    let provider = InMemoryProvider {
        edges: Vec::new(),
        failure: Some("index is unavailable".into()),
    };
    let result = compiler::compile(
        temp.path(),
        &config::parse(CONFIG).unwrap(),
        &provider,
        false,
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
        ],
    )
    .await;
    assert_eq!(ir.stats.observed_edge_count, 2);
    assert_eq!(ir.stats.resolved_edge_count, 0);
    assert_eq!(ir.diagnostics.provider_anomalies.len(), 2);
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
    let ir = compiler::compile(temp.path(), &validated, &InMemoryProvider::default(), false)
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
    };
    let ir = compiler::compile(
        temp.path(),
        &config::parse(&yaml).unwrap(),
        &provider,
        false,
    )
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
}
