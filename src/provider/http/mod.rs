//! HTTP observations. GitNexus finds server routes (`Route` nodes, e.g. from
//! FastAPI decorators) but links a client to them only for
//! `fetch('/literal')` and a few wrapper names (docs/gitnexus-limitations.md).
//! ArchGraph reads client calls itself and matches them to the provider's
//! routes: a `FETCHES` edge goes from the calling file to the file handling
//! the route, as GitNexus's own `FETCHES` relations do.
pub mod client;

use crate::{
    model::{
        CallProblem, ClientCall, CodeEdge, HttpReport, Route, RouteUse, SourceLine, UnresolvedCall,
    },
    provider::typescript::is_script,
};
use anyhow::{Context, Result};
use client::{Piece, Via};
use std::{collections::BTreeSet, path::Path};

pub const KIND: &str = "FETCHES";

/// Stands for a runtime part while a URL is split into segments.
const RUNTIME: char = '\u{0}';

#[derive(Debug, Default)]
pub struct Extraction {
    pub edges: Vec<CodeEdge>,
    pub report: HttpReport,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy)]
enum Wanted<'a> {
    Literal(&'a str),
    Runtime,
}

enum Declared<'a> {
    Literal(&'a str),
    Parameter,
    /// `{path:path}`, `*`: the rest of the URL.
    Rest,
}

fn declared(path: &str) -> Vec<Declared<'_>> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            if segment.starts_with('*') || (segment.starts_with('{') && segment.ends_with(":path}"))
            {
                Declared::Rest
            } else if segment.starts_with('{') || segment.starts_with(':') {
                Declared::Parameter
            } else {
                Declared::Literal(segment)
            }
        })
        .collect()
}

/// The path of a call's URL with runtime parts marked, and whether the URL
/// names a host.
fn wanted(url: &str) -> (Vec<Wanted<'_>>, bool) {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let (path, absolute) = match path.split_once("//") {
        Some((scheme, rest)) if scheme.is_empty() || scheme.ends_with(':') => {
            (rest.find('/').map_or("", |slash| &rest[slash..]), true)
        }
        _ => (path, false),
    };
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .map(|segment| {
            if segment.contains(RUNTIME) {
                Wanted::Runtime
            } else {
                Wanted::Literal(segment)
            }
        })
        .collect();
    (segments, absolute)
}

/// How specific a match is: the number of literal route segments it
/// matched. A route of parameters only (a single-page app's catch-all)
/// matches nothing.
fn score(wanted: &[Wanted], declared: &[Declared]) -> Option<usize> {
    let mut literals = 0;
    for (index, segment) in declared.iter().enumerate() {
        match (segment, wanted.get(index)) {
            (Declared::Rest, _) => return (literals > 0).then_some(literals),
            (Declared::Literal(expected), Some(Wanted::Literal(actual))) if expected == actual => {
                literals += 1
            }
            (Declared::Parameter, Some(_)) => {}
            _ => return None,
        }
    }
    (wanted.len() == declared.len() && literals > 0).then_some(literals)
}

fn display(url: &[Piece]) -> String {
    url.iter()
        .map(|piece| match piece {
            Piece::Text(text) => text.clone(),
            Piece::Param => RUNTIME.to_string(),
        })
        .collect()
}

pub fn extract(root: &Path, files: &[String], routes: &[Route]) -> Result<Extraction> {
    let mut extraction = Extraction::default();
    let mut uses: Vec<BTreeSet<SourceLine>> = vec![BTreeSet::new(); routes.len()];
    let declared_routes: Vec<Vec<Declared>> = routes.iter().map(|r| declared(&r.path)).collect();
    for path in files.iter().filter(|path| is_script(path)) {
        let source = std::fs::read_to_string(root.join(path))
            .with_context(|| format!("cannot read {path}"))?;
        let facts = client::parse(path, &source)?;
        extraction.warnings.extend(facts.warnings);
        extraction
            .report
            .unresolved
            .extend(
                facts
                    .unresolved
                    .into_iter()
                    .map(|unresolved| UnresolvedCall {
                        file: path.clone(),
                        line: unresolved.line,
                        expression: unresolved.expression,
                    }),
            );
        for call in facts.calls {
            let url = display(&call.url);
            let (wanted, absolute) = wanted(&url);
            let matching: Vec<(usize, usize)> = declared_routes
                .iter()
                .enumerate()
                .filter_map(|(index, declared)| Some((index, score(&wanted, declared)?)))
                .collect();
            let accepting: Vec<(usize, usize)> = matching
                .iter()
                .copied()
                .filter(|(index, _)| match (&call.method, &routes[*index].method) {
                    (Some(called), Some(accepted)) => called == accepted,
                    _ => true,
                })
                .collect();
            let problem = match (matching.is_empty(), accepting.is_empty()) {
                (true, _) if absolute => Some(CallProblem::External),
                (true, _) => Some(CallProblem::NoRoute),
                (false, true) => Some(CallProblem::WrongMethod),
                (false, false) => None,
            };
            if let Some(problem) = problem {
                extraction.report.unmatched.push(ClientCall {
                    file: path.clone(),
                    line: call.line,
                    method: call.method,
                    url: url.replace(RUNTIME, "{}"),
                    problem,
                });
                continue;
            }
            let best = accepting.iter().map(|(_, score)| *score).max();
            let (reason, confidence) = match call.via {
                Via::Call => ("http-call", 1.0),
                Via::Wrapper => ("http-wrapper", 1.0),
                Via::Link => ("http-link", 0.8),
            };
            for (index, _) in accepting.iter().filter(|(_, score)| Some(*score) == best) {
                uses[*index].insert(SourceLine {
                    file: path.clone(),
                    line: call.line,
                });
                let handler = &routes[*index].file;
                if handler != path {
                    extraction.edges.push(CodeEdge {
                        from_file: path.clone(),
                        to_file: handler.clone(),
                        kind: KIND.into(),
                        confidence: Some(confidence),
                        reason: Some(reason.into()),
                    });
                }
            }
        }
    }
    let mut report_routes: Vec<RouteUse> = routes
        .iter()
        .cloned()
        .zip(uses)
        .map(|(route, callers)| RouteUse {
            route,
            callers: callers.into_iter().collect(),
        })
        .collect();
    report_routes.sort_by(|a, b| {
        let key = |r: &Route| (r.file.clone(), r.path.clone(), r.method.clone());
        key(&a.route).cmp(&key(&b.route))
    });
    extraction.report.routes = report_routes;
    extraction.report.unmatched.sort();
    extraction.report.unresolved.sort();
    Ok(extraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(method: &str, path: &str) -> Route {
        Route {
            method: Some(method.into()),
            path: path.into(),
            file: "backend/app.py".into(),
        }
    }

    #[test]
    fn calls_reach_the_most_specific_route_with_their_method() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("web")).unwrap();
        std::fs::write(
            root.join("web/api.ts"),
            "const BASE = '/api'\n\
             fetch(`${BASE}/plans/jobs`, { method: 'POST' })\n\
             fetch(`${BASE}/plans/${id}`)\n\
             fetch(`${BASE}/plans/${id}`, { method: 'DELETE' })\n\
             fetch('/api/nothing')\n\
             fetch('https://tiles.example.org/x')\n\
             fetch(url)\n",
        )
        .unwrap();
        let routes = [
            route("POST", "/api/plans/jobs"),
            route("GET", "/api/plans/{plan_id}"),
            route("GET", "/{path:path}"),
        ];
        let extraction = extract(root, &["web/api.ts".to_owned()], &routes).unwrap();
        let callers: Vec<(&str, Vec<usize>)> = extraction
            .report
            .routes
            .iter()
            .map(|use_| {
                (
                    use_.route.path.as_str(),
                    use_.callers.iter().map(|caller| caller.line).collect(),
                )
            })
            .collect();
        assert_eq!(
            callers,
            [
                ("/api/plans/jobs", vec![2]),
                ("/api/plans/{plan_id}", vec![3]),
                ("/{path:path}", vec![]),
            ]
        );
        let unmatched: Vec<(usize, &str, CallProblem)> = extraction
            .report
            .unmatched
            .iter()
            .map(|call| (call.line, call.url.as_str(), call.problem))
            .collect();
        assert_eq!(
            unmatched,
            [
                (4, "/api/plans/{}", CallProblem::WrongMethod),
                (5, "/api/nothing", CallProblem::NoRoute),
                (6, "https://tiles.example.org/x", CallProblem::External),
            ]
        );
        assert_eq!(extraction.report.unresolved[0].expression, "url");
        assert_eq!(extraction.edges.len(), 2);
        assert!(extraction
            .edges
            .iter()
            .all(|edge| edge.to_file == "backend/app.py" && edge.kind == KIND));
    }
}
