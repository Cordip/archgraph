use crate::{
    model::{ArchitectureIr, EVIDENCE_LIMIT},
    projection::{self, NodeSummary, Projection},
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
    net::{IpAddr, SocketAddr},
    sync::{Arc, RwLock},
};

/// The architecture being served. `serve` replaces it when the index or the
/// configuration changes; every request reads one complete IR.
#[derive(Debug)]
pub struct Live {
    state: RwLock<LiveState>,
    watching: bool,
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
        })
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
        .route("/app.js", get(javascript))
        .route("/style.css", get(stylesheet))
        .route("/api/meta", get(meta))
        .route("/api/nodes", get(nodes))
        .route("/api/focus/{node_id}", get(focus))
        .route("/api/violations", get(violations))
        .route("/api/search", get(search))
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
async fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(
            json!({"error":"not found; this server exposes read-only architecture metadata, not Cypher or source files"}),
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
