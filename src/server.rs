use crate::{
    model::{ArchitectureIr, EVIDENCE_LIMIT},
    projection::{self, NodeSummary, Projection},
    snapshot, source, vcs,
};
use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
    path::{Path as FilePath, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::SystemTime,
};

/// The architecture being served. `serve` replaces it when the index or the
/// configuration changes; every request reads one complete IR.
#[derive(Debug)]
pub struct Live {
    state: RwLock<LiveState>,
    watching: bool,
    /// The canonical repository root, when the server may show the source
    /// of mapped files (`GET /api/source`); `None` refuses every request.
    /// Snapshots (`GET /api/diff`) are read from under it too.
    source_root: Option<PathBuf>,
    /// The snapshot last compared with, so that moving around the levels
    /// does not parse it (tens of megabytes on zammad) and ask Git about
    /// renames again for every request.
    compared: Mutex<Option<Compared>>,
}

#[derive(Debug)]
struct Compared {
    name: String,
    /// The snapshot file's size and modification time when it was read.
    stamp: (u64, SystemTime),
    /// The live revision the renames were found for.
    revision: u64,
    snapshot: Arc<snapshot::Snapshot>,
    renames: Arc<BTreeMap<String, String>>,
    rename_error: Option<String>,
}

#[derive(Debug)]
struct LiveState {
    ir: Arc<ArchitectureIr>,
    revision: u64,
    refresh_error: Option<String>,
}

impl Live {
    /// `watching`: whether anything will ever call `publish` or `fail`.
    pub fn new(ir: ArchitectureIr, watching: bool) -> Arc<Self> {
        Arc::new(Self {
            state: RwLock::new(LiveState {
                ir: Arc::new(ir),
                revision: 1,
                refresh_error: None,
            }),
            watching,
            source_root: None,
            compared: Mutex::new(None),
        })
    }
    /// Like `new`, and serves the source of mapped files under `root`.
    pub fn with_sources(ir: ArchitectureIr, watching: bool, root: &FilePath) -> Result<Arc<Self>> {
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot resolve repository root {}", root.display()))?;
        Ok(Arc::new(Self {
            state: RwLock::new(LiveState {
                ir: Arc::new(ir),
                revision: 1,
                refresh_error: None,
            }),
            watching,
            source_root: Some(root),
            compared: Mutex::new(None),
        }))
    }
    fn read(&self) -> std::sync::RwLockReadGuard<'_, LiveState> {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, LiveState> {
        self.state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    pub fn current(&self) -> Arc<ArchitectureIr> {
        self.read().ir.clone()
    }
    pub fn revision(&self) -> u64 {
        self.read().revision
    }
    pub fn publish(&self, ir: ArchitectureIr) -> u64 {
        let mut state = self.write();
        state.ir = Arc::new(ir);
        state.revision += 1;
        state.refresh_error = None;
        state.revision
    }
    /// Keeps serving the last good IR, and says why it is not newer.
    pub fn fail(&self, error: String) {
        self.write().refresh_error = Some(error);
    }
}

/// A fixed snapshot that never refreshes.
pub fn router(ir: Arc<ArchitectureIr>) -> Router {
    let ir = Arc::try_unwrap(ir).unwrap_or_else(|shared| (*shared).clone());
    live_router(Live::new(ir, false))
}

pub fn live_router(live: Arc<Live>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/board.js", get(board_javascript))
        .route("/viewer.js", get(viewer_javascript))
        .route("/dsm.js", get(dsm_javascript))
        .route("/app.js", get(javascript))
        .route("/style.css", get(stylesheet))
        .route("/api/meta", get(meta))
        .route("/api/nodes", get(nodes))
        .route("/api/focus/{node_id}", get(focus))
        .route("/api/violations", get(violations))
        .route("/api/search", get(search))
        .route("/api/packages", get(packages))
        .route("/api/source", get(source))
        .route("/api/snapshots", get(snapshots))
        .route("/api/diff/{node_id}", get(diff))
        .fallback(not_found)
        .layer(middleware::map_response(security_headers))
        .with_state(live)
}

pub async fn serve(live: Arc<Live>, host: IpAddr, port: u16) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(host, port))
        .await
        .with_context(|| {
            format!("cannot bind {host}:{port}; choose another --port or a valid local --host")
        })?;
    eprintln!(
        "ArchGraph: http://{} ({}; Ctrl-C to stop)",
        listener.local_addr()?,
        if live.watching {
            "read-only, reloads when the index or configuration changes"
        } else {
            "read-only snapshot"
        }
    );
    axum::serve(listener, live_router(live))
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("shutdown signal failed: {error}");
            }
        })
        .await
        .context("ArchGraph HTTP server failed")
}

async fn index() -> Html<&'static str> {
    Html(include_str!("web/index.html"))
}
async fn javascript() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("web/app.js"),
    )
}
async fn board_javascript() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("web/board.js"),
    )
}
async fn viewer_javascript() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("web/viewer.js"),
    )
}
async fn dsm_javascript() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("web/dsm.js"),
    )
}
async fn stylesheet() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("web/style.css"),
    )
}
async fn meta(State(live): State<Arc<Live>>) -> Json<Value> {
    let state = live.read();
    let ir = &state.ir;
    Json(
        json!({"project":ir.project, "provider":ir.provider, "stats":ir.stats,
        "schema_version":ir.schema_version, "evidence_notice":ir.evidence_notice,
        "diagnostics":ir.diagnostics.warnings, "read_only":true,
        "watching":live.watching, "revision":state.revision, "refresh_error":state.refresh_error}),
    )
}
async fn nodes(State(live): State<Arc<Live>>) -> Json<Vec<NodeSummary>> {
    let ir = live.current();
    Json(ir.nodes.values().map(NodeSummary::from).collect())
}
async fn focus(
    State(live): State<Arc<Live>>,
    Path(id): Path<String>,
) -> std::result::Result<Json<Projection>, (StatusCode, Json<Value>)> {
    let ir = live.current();
    if !ir.nodes.contains_key(&id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error":format!("unknown architecture node `{id}`; use search")})),
        ));
    }
    projection::project(&ir, &id, EVIDENCE_LIMIT)
        .map(Json)
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":error.to_string()})),
            )
        })
}
async fn violations(State(live): State<Arc<Live>>) -> Json<Value> {
    let ir = live.current();
    Json(json!({"violations":ir.violations}))
}
#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}
async fn search(
    State(live): State<Arc<Live>>,
    Query(query): Query<SearchQuery>,
) -> Json<Vec<NodeSummary>> {
    let ir = live.current();
    let query = query.q.trim().to_lowercase();
    Json(
        ir.nodes
            .values()
            .filter(|node| {
                node.id.to_lowercase().contains(&query)
                    || node.title.to_lowercase().contains(&query)
            })
            .take(50)
            .map(NodeSummary::from)
            .collect(),
    )
}
/// Every imported package (`provider.packages`) with the node owning it and
/// the nodes importing it, for search; the importing files and lines come
/// with the package's entry in its owner's focus.
async fn packages(State(live): State<Arc<Live>>) -> Json<Value> {
    let ir = live.current();
    let packages: Vec<Value> = ir
        .packages
        .iter()
        .flat_map(|report| &report.packages)
        .map(|package| {
            let files: BTreeSet<&str> = package.imports.iter().map(|i| i.file.as_str()).collect();
            let nodes: BTreeSet<&str> = package
                .imports
                .iter()
                .filter_map(|import| import.node.as_deref())
                .collect();
            // The import lines let the UI name each package on an edge with
            // the file and line that imports it.
            json!({"id": package.id, "name": package.name, "ecosystem": package.ecosystem,
                "node": package.node, "file_count": files.len(), "nodes": nodes,
                "imports": package.imports})
        })
        .collect();
    Json(json!({"enabled": ir.packages.is_some(), "packages": packages}))
}
#[derive(Deserialize)]
struct SourceQuery {
    #[serde(default)]
    path: String,
    /// The hash of the text the viewer shows; the text is left out when it
    /// still matches.
    if_hash: Option<String>,
}
/// The source of one mapped file; see `source::read_mapped` for the scope.
async fn source(
    State(live): State<Arc<Live>>,
    Query(query): Query<SourceQuery>,
) -> std::result::Result<Json<source::SourceFile>, (StatusCode, Json<Value>)> {
    let refused = |status: u16, reason: &str, message: String| {
        (
            StatusCode::from_u16(status).unwrap_or(StatusCode::FORBIDDEN),
            Json(json!({"error": message, "reason": reason})),
        )
    };
    let Some(root) = live.source_root.clone() else {
        return Err(refused(
            404,
            "disabled",
            "this server shows no source files; run archgraph serve to view mapped files".into(),
        ));
    };
    let ir = live.current();
    let read = tokio::task::spawn_blocking(move || {
        source::read_mapped(&ir, &root, &query.path, query.if_hash.as_deref())
    })
    .await
    .map_err(|error| refused(500, "internal", format!("reading the file failed: {error}")))?;
    read.map(Json)
        .map_err(|refusal| refused(refusal.status, refusal.reason, refusal.message))
}
type Refusal = (StatusCode, Json<Value>);

fn refusal(status: StatusCode, reason: &str, message: String) -> Refusal {
    (status, Json(json!({"error": message, "reason": reason})))
}

/// Saved snapshots, for the UI's comparison menu.
async fn snapshots(State(live): State<Arc<Live>>) -> std::result::Result<Json<Value>, Refusal> {
    let Some(root) = live.source_root.clone() else {
        return Ok(Json(json!({"enabled": false, "snapshots": []})));
    };
    let listed = tokio::task::spawn_blocking(move || snapshot::list(&root))
        .await
        .map_err(|error| {
            refusal(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                error.to_string(),
            )
        })?
        .map_err(|error| {
            refusal(
                StatusCode::INTERNAL_SERVER_ERROR,
                "unreadable",
                format!("{error:#}"),
            )
        })?;
    Ok(Json(json!({"enabled": true, "snapshots": listed})))
}

#[derive(Deserialize)]
struct DiffQuery {
    #[serde(default)]
    snapshot: String,
}

/// The snapshot and the renames since it, read again only when the file or
/// the live revision changed.
/// A snapshot, the renames since it (old -> new path) and why they could
/// not be found.
type Comparison = (
    Arc<snapshot::Snapshot>,
    Arc<BTreeMap<String, String>>,
    Option<String>,
);

fn compared(
    live: &Live,
    root: &FilePath,
    name: &str,
    ir: &ArchitectureIr,
    revision: u64,
) -> std::result::Result<Comparison, Refusal> {
    let path = snapshot::path(root, name)
        .map_err(|error| refusal(StatusCode::BAD_REQUEST, "invalid", format!("{error:#}")))?;
    let stamp = std::fs::metadata(&path)
        .and_then(|metadata| Ok((metadata.len(), metadata.modified()?)))
        .map_err(|_| {
            refusal(
                StatusCode::NOT_FOUND,
                "missing",
                format!(
                    "no snapshot named `{name}`; take one with `archgraph snapshot --name {name}`"
                ),
            )
        })?;
    let mut cache = live
        .compared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(cached) = cache.as_ref() {
        if cached.name == name && cached.stamp == stamp && cached.revision == revision {
            return Ok((
                cached.snapshot.clone(),
                cached.renames.clone(),
                cached.rename_error.clone(),
            ));
        }
    }
    let loaded = match cache.as_ref() {
        Some(cached) if cached.name == name && cached.stamp == stamp => cached.snapshot.clone(),
        _ => Arc::new(snapshot::load(root, name).map_err(|error| {
            refusal(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unreadable",
                format!("{error:#}"),
            )
        })?),
    };
    // Git is asked only when files differ: a rename needs a file gone and one new.
    let files_differ = !loaded
        .ir
        .files
        .iter()
        .map(|file| &file.path)
        .eq(ir.files.iter().map(|file| &file.path));
    let (renames, rename_error) = match &loaded.commit {
        Some(commit) if files_differ => match vcs::renames_since(root, commit) {
            Ok(renames) => (renames, None),
            Err(error) => (
                BTreeMap::new(),
                Some(format!(
                    "moved files are shown as removed and added: {error:#}"
                )),
            ),
        },
        _ => (BTreeMap::new(), None),
    };
    let renames = Arc::new(renames);
    *cache = Some(Compared {
        name: name.to_owned(),
        stamp,
        revision,
        snapshot: loaded.clone(),
        renames: renames.clone(),
        rename_error: rename_error.clone(),
    });
    Ok((loaded, renames, rename_error))
}

/// One level compared with the same level in a snapshot.
async fn diff(
    State(live): State<Arc<Live>>,
    Path(id): Path<String>,
    Query(query): Query<DiffQuery>,
) -> std::result::Result<Json<Value>, Refusal> {
    let Some(root) = live.source_root.clone() else {
        return Err(refusal(
            StatusCode::NOT_FOUND,
            "disabled",
            "this server has no snapshots; run archgraph serve in the repository".into(),
        ));
    };
    let (ir, revision) = {
        let state = live.read();
        (state.ir.clone(), state.revision)
    };
    if !ir.nodes.contains_key(&id) {
        return Err(refusal(
            StatusCode::NOT_FOUND,
            "unknown_node",
            format!("unknown architecture node `{id}`; use search"),
        ));
    }
    let name = if query.snapshot.is_empty() {
        snapshot::DEFAULT_NAME.to_owned()
    } else {
        query.snapshot
    };
    let worker = live.clone();
    tokio::task::spawn_blocking(move || {
        let (saved, renames, warning) = compared(&worker, &root, &name, &ir, revision)?;
        let level = snapshot::level_diff(&saved, &ir, &id, &renames).map_err(|error| {
            refusal(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                format!("{error:#}"),
            )
        })?;
        let mut value = serde_json::to_value(level).map_err(|error| {
            refusal(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                error.to_string(),
            )
        })?;
        value["revision"] = json!(revision);
        value["warning"] = json!(warning);
        Ok(Json(value))
    })
    .await
    .map_err(|error| {
        refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            error.to_string(),
        )
    })?
}
async fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(
            json!({"error":"not found; this server exposes read-only architecture metadata and the source of mapped files (/api/source), not Cypher"}),
        ),
    )
}
async fn security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
