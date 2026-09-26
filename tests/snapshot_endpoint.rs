//! Snapshots through the server: `GET /api/snapshots` and
//! `GET /api/diff/{node}`, which compare one level of a saved compile with
//! the same level of the live one.
use archgraph::{
    compiler, config,
    model::{ArchitectureIr, CodeEdge},
    provider::InMemoryProvider,
    server, snapshot,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use serde_json::Value;
use std::path::Path;
use tower::ServiceExt;

const CONFIG: &str = include_str!("fixtures/architecture.yaml");

fn repository() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    for path in [
        "src/api/routes.py",
        "src/domain/model.py",
        "src/persistence/db.py",
        "src/shared/types.py",
    ] {
        let path = temp.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "pass\n").unwrap();
    }
    temp
}

fn edge(from: &str, to: &str) -> CodeEdge {
    CodeEdge {
        from_file: from.into(),
        to_file: to.into(),
        kind: "IMPORTS".into(),
        confidence: Some(1.0),
        reason: Some("fixture".into()),
    }
}

async fn compile(root: &Path, edges: Vec<CodeEdge>) -> ArchitectureIr {
    compiler::compile(
        root,
        &config::parse(CONFIG).unwrap(),
        &InMemoryProvider {
            edges,
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// The API used the domain; now the domain uses persistence, which a rule
/// denies.
async fn changed_repository() -> (tempfile::TempDir, Router) {
    let temp = repository();
    let before = compile(
        temp.path(),
        vec![edge("src/api/routes.py", "src/domain/model.py")],
    )
    .await;
    snapshot::save(temp.path(), "before", &before, None).unwrap();

    let after = compile(
        temp.path(),
        vec![edge("src/domain/model.py", "src/persistence/db.py")],
    )
    .await;
    let live = server::Live::with_sources(after, false, temp.path()).unwrap();
    (temp, server::live_router(live))
}

fn keys(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| {
            format!(
                "{} -> {}",
                item["from"].as_str().unwrap(),
                item["to"].as_str().unwrap()
            )
        })
        .collect()
}

#[tokio::test]
async fn a_level_diff_names_added_and_removed_edges_and_violations() {
    let (_temp, app) = changed_repository().await;

    let (status, body) = get(&app, "/api/diff/app?snapshot=before").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["focus_existed"], true);
    assert_eq!(
        keys(&body["edges_added"]),
        ["node:app.domain -> node:app.persistence"]
    );
    assert_eq!(
        keys(&body["edges_removed"]),
        ["node:app.api -> node:app.domain"]
    );
    let appeared: Vec<&str> = body["violations_appeared"]
        .as_array()
        .unwrap()
        .iter()
        .map(|violation| violation["rule_id"].as_str().unwrap())
        .collect();
    assert!(appeared.contains(&"deny-storage"), "{body}");
    assert_eq!(body["violations_resolved"], serde_json::json!([]));
    assert_eq!(
        body["summary"]["dependencies_added"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        body["summary"]["dependencies_removed"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(body["entries_added"], serde_json::json!([]));
}

#[tokio::test]
async fn saved_snapshots_are_listed_and_the_default_name_is_before() {
    let (_temp, app) = changed_repository().await;

    let (status, listed) = get(&app, "/api/snapshots").await;
    let (default_status, _) = get(&app, "/api/diff/app").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["enabled"], true);
    assert_eq!(listed["snapshots"][0]["name"], "before");
    assert_eq!(default_status, StatusCode::OK);
}

#[tokio::test]
async fn bad_names_missing_snapshots_and_unknown_nodes_are_refused() {
    let (_temp, app) = changed_repository().await;

    let cases = [
        ("/api/diff/app?snapshot=..%2Fsecret", 400, "invalid"),
        ("/api/diff/app?snapshot=.hidden", 400, "invalid"),
        ("/api/diff/app?snapshot=nothing", 404, "missing"),
        ("/api/diff/app.nothing?snapshot=before", 404, "unknown_node"),
    ];

    for (uri, status, reason) in cases {
        let (got, body) = get(&app, uri).await;
        assert_eq!(got.as_u16(), status, "{uri}: {body}");
        assert_eq!(body["reason"], reason, "{uri}: {body}");
    }
}

#[tokio::test]
async fn a_server_without_a_repository_has_no_snapshots() {
    let temp = repository();
    let ir = compile(temp.path(), Vec::new()).await;
    let app = server::router(std::sync::Arc::new(ir));

    let (_, listed) = get(&app, "/api/snapshots").await;
    let (status, body) = get(&app, "/api/diff/app").await;

    assert_eq!(listed["enabled"], false);
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["reason"], "disabled");
}

#[tokio::test]
async fn a_level_the_snapshot_did_not_have_is_all_new() {
    let temp = repository();
    let before = compile(temp.path(), Vec::new()).await;
    let mut older = before.clone();
    older.nodes.remove("app.shared");
    snapshot::save(temp.path(), "before", &older, None).unwrap();
    let live = server::Live::with_sources(before, false, temp.path()).unwrap();
    let app = server::live_router(live);

    let (status, body) = get(&app, "/api/diff/app.shared").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["focus_existed"], false);
    assert_eq!(
        body["entries_added"],
        serde_json::json!(["file:src/shared/types.py"])
    );
}
