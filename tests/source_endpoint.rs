//! `GET /api/source`: the one endpoint that reads repository files. Each
//! refusal is asserted by its reason code, so removing any single check
//! turns its test from a refusal into another reason or a success.
use archgraph::{
    compiler, config,
    model::{ArchitectureIr, CodeEdge},
    provider::InMemoryProvider,
    server,
    source::SOURCE_SIZE_LIMIT,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use std::{path::Path, sync::Arc};
use tower::ServiceExt;

const CONFIG: &str = include_str!("fixtures/architecture.yaml");
const SOURCE: &str = "import domain.model\n\nprint('fixture')\n";

fn write(root: &Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
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
        write(temp.path(), path, SOURCE.as_bytes());
    }
    // On disk but outside `source_roots`: never mapped.
    write(temp.path(), "notes.txt", b"private notes\n");
    temp
}
async fn compile(root: &Path) -> ArchitectureIr {
    let edge = CodeEdge {
        from_file: "src/api/routes.py".into(),
        to_file: "src/domain/model.py".into(),
        kind: "IMPORTS".into(),
        confidence: Some(0.9),
        reason: Some("fixture".into()),
    };
    compiler::compile(
        root,
        &config::parse(CONFIG).unwrap(),
        &InMemoryProvider {
            edges: vec![edge],
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap()
}
async fn app(root: &Path) -> Router {
    let ir = compile(root).await;
    server::live_router(server::Live::with_sources(ir, false, root).unwrap())
}
async fn get(app: &Router, path: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/source?path={path}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert!(response.headers().contains_key("content-security-policy"));
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
/// The status and reason of a refusal, after checking it explains itself.
async fn refusal(app: &Router, path: &str) -> (u16, String) {
    let (status, body) = get(app, path).await;
    assert!(body.get("text").is_none(), "{path}: {body}");
    assert!(
        body["error"].as_str().is_some_and(|error| error.len() > 20),
        "{path}: {body}"
    );
    (
        status.as_u16(),
        body["reason"].as_str().unwrap_or("").to_owned(),
    )
}

#[tokio::test]
async fn serves_a_mapped_file_as_text_with_its_line_count_and_hash() {
    let temp = repository();
    let app = app(temp.path()).await;
    let (status, body) = get(&app, "src/domain/model.py").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["path"], "src/domain/model.py");
    assert_eq!(body["node"], "app.domain");
    assert_eq!(body["text"], SOURCE);
    assert_eq!(body["line_count"], 3);
    assert_eq!(body["bytes"], SOURCE.len());
    assert_eq!(body["unchanged"], false);
    let hash = body["hash"].as_str().unwrap().to_owned();
    assert_eq!(hash.len(), 16);

    // The viewer asks again with the hash it shows: no text while unchanged.
    let (_, again) = get(&app, &format!("src/domain/model.py&if_hash={hash}")).await;
    assert_eq!(
        (again["unchanged"].as_bool(), again.get("text")),
        (Some(true), None)
    );
    write(temp.path(), "src/domain/model.py", b"changed\n");
    let (_, changed) = get(&app, &format!("src/domain/model.py&if_hash={hash}")).await;
    assert_eq!(changed["unchanged"], false);
    assert_eq!(changed["text"], "changed\n");
    assert_ne!(changed["hash"], hash);
}

#[tokio::test]
async fn refuses_paths_that_are_not_a_mapped_files_own_relative_path() {
    let temp = repository();
    let app = app(temp.path()).await;
    let absolute = format!("{}/src/domain/model.py", temp.path().display());
    assert_eq!(refusal(&app, "").await, (400, "empty".into()));
    assert_eq!(refusal(&app, &absolute).await, (400, "absolute".into()));
    assert_eq!(refusal(&app, "/etc/passwd").await, (400, "absolute".into()));
    assert_eq!(
        refusal(&app, "C:/Windows/win.ini").await,
        (400, "absolute".into())
    );
    assert_eq!(
        refusal(&app, "src/domain/../domain/model.py").await,
        (400, "parent".into())
    );
    assert_eq!(refusal(&app, "../notes.txt").await, (400, "parent".into()));
    assert_eq!(
        refusal(&app, "src/./domain/model.py").await,
        (400, "not_normalized".into())
    );
    // Exists on disk, but the architecture does not map it.
    assert_eq!(refusal(&app, "notes.txt").await, (403, "unmapped".into()));
    assert_eq!(refusal(&app, "src").await, (403, "unmapped".into()));
    assert_eq!(refusal(&app, ".git/config").await, (403, "unmapped".into()));
}

#[tokio::test]
async fn refuses_mapped_files_that_are_no_longer_regular_utf8_text_within_the_limit() {
    let temp = repository();
    let app = app(temp.path()).await;
    // Each file was a small text file when the IR was compiled.
    std::fs::remove_file(temp.path().join("src/api/routes.py")).unwrap();
    std::fs::create_dir(temp.path().join("src/api/routes.py")).unwrap();
    assert_eq!(
        refusal(&app, "src/api/routes.py").await,
        (403, "not_file".into())
    );

    std::fs::remove_file(temp.path().join("src/main.py")).unwrap();
    assert_eq!(refusal(&app, "src/main.py").await, (404, "missing".into()));

    write(temp.path(), "src/domain/model.py", b"text\0with a NUL\n");
    assert_eq!(
        refusal(&app, "src/domain/model.py").await,
        (415, "binary".into())
    );

    write(temp.path(), "src/shared/types.py", b"caf\xe9 in Latin-1\n");
    assert_eq!(
        refusal(&app, "src/shared/types.py").await,
        (415, "not_utf8".into())
    );

    let size = SOURCE_SIZE_LIMIT as usize;
    write(
        temp.path(),
        "src/api/handlers/route.py",
        &vec![b'#'; size + 1],
    );
    assert_eq!(
        refusal(&app, "src/api/handlers/route.py").await,
        (413, "too_large".into())
    );
    write(temp.path(), "src/api/handlers/route.py", &vec![b'#'; size]);
    let (status, body) = get(&app, "src/api/handlers/route.py").await;
    assert_eq!(
        (status, body["bytes"].as_u64()),
        (StatusCode::OK, Some(size as u64))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_symlinks_and_paths_resolving_outside_the_repository() {
    use std::os::unix::fs::symlink;
    let temp = repository();
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "db.py", b"secret outside the repository\n");
    let app = app(temp.path()).await;
    // The file itself became a symlink, even to a file inside the root.
    std::fs::remove_file(temp.path().join("src/shared/types.py")).unwrap();
    symlink(
        temp.path().join("src/main.py"),
        temp.path().join("src/shared/types.py"),
    )
    .unwrap();
    assert_eq!(
        refusal(&app, "src/shared/types.py").await,
        (403, "symlink".into())
    );
    // A directory on the way became a symlink out of the repository.
    std::fs::remove_dir_all(temp.path().join("src/persistence")).unwrap();
    symlink(outside.path(), temp.path().join("src/persistence")).unwrap();
    assert_eq!(
        refusal(&app, "src/persistence/db.py").await,
        (403, "outside".into())
    );
}

#[tokio::test]
async fn a_snapshot_router_serves_no_source() {
    let temp = repository();
    let app = server::router(Arc::new(compile(temp.path()).await));
    assert_eq!(
        refusal(&app, "src/domain/model.py").await,
        (404, "disabled".into())
    );
}
