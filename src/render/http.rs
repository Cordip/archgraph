//! `archgraph http`: which client calls reach which routes, whole or for one
//! node's subtree.
use super::Owners;
use crate::model::{ArchitectureIr, CallProblem, ClientCall, HttpReport, RouteUse};
use std::{collections::BTreeMap, fmt::Write};

/// The part of the report that touches `node`: routes handled or called in
/// its subtree (with all their callers, so outside callers stay visible),
/// and calls made in it.
pub fn select(ir: &ArchitectureIr, report: &HttpReport, node: Option<&str>) -> HttpReport {
    let owners = Owners::new(ir);
    HttpReport {
        routes: report
            .routes
            .iter()
            .filter(|use_| {
                owners.within(&use_.route.file, node)
                    || use_.callers.iter().any(|c| owners.within(&c.file, node))
            })
            .cloned()
            .collect(),
        unmatched: report
            .unmatched
            .iter()
            .filter(|call| owners.within(&call.file, node))
            .cloned()
            .collect(),
        unresolved: report
            .unresolved
            .iter()
            .filter(|call| owners.within(&call.file, node))
            .cloned()
            .collect(),
    }
}

fn method(method: Option<&str>) -> &str {
    method.unwrap_or("ANY")
}

fn calls(out: &mut String, title: &str, calls: &[&ClientCall]) {
    if calls.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{title} ({}):", calls.len());
    for call in calls {
        let problem = match call.problem {
            CallProblem::WrongMethod => "  (the route does not accept this method)",
            CallProblem::NoRoute | CallProblem::External => "",
        };
        let _ = writeln!(
            out,
            "  {:<6} {}  {}:{}{problem}",
            method(call.method.as_deref()),
            call.url,
            call.file,
            call.line
        );
    }
}

pub fn render(report: &HttpReport, node: Option<&str>) -> String {
    let mut out = String::new();
    let scope = node.map_or(String::new(), |node| format!(" touching `{node}`"));
    let called = report
        .routes
        .iter()
        .filter(|r| !r.callers.is_empty())
        .count();
    let _ = writeln!(
        out,
        "Routes{scope}: {} ({called} called by client code)",
        report.routes.len()
    );
    let mut by_handler: BTreeMap<&str, Vec<&RouteUse>> = BTreeMap::new();
    for use_ in &report.routes {
        by_handler.entry(&use_.route.file).or_default().push(use_);
    }
    for (handler, routes) in by_handler {
        let _ = writeln!(out, "\n{handler}:");
        let width = routes.iter().map(|r| r.route.path.len()).max().unwrap_or(0);
        for use_ in routes {
            let callers = if use_.callers.is_empty() {
                "not called".to_owned()
            } else {
                use_.callers
                    .iter()
                    .map(|caller| format!("{}:{}", caller.file, caller.line))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = writeln!(
                out,
                "  {:<6} {:width$}  {callers}",
                method(use_.route.method.as_deref()),
                use_.route.path
            );
        }
    }
    let of = |wanted: &[CallProblem]| -> Vec<&ClientCall> {
        report
            .unmatched
            .iter()
            .filter(|call| wanted.contains(&call.problem))
            .collect()
    };
    calls(
        &mut out,
        "Calls that reach no route",
        &of(&[CallProblem::NoRoute, CallProblem::WrongMethod]),
    );
    calls(
        &mut out,
        "External calls: absolute URLs no route matches",
        &of(&[CallProblem::External]),
    );
    if !report.unresolved.is_empty() {
        let _ = writeln!(
            out,
            "\nUnresolved calls ({}); their URLs are not known statically:",
            report.unresolved.len()
        );
        for call in &report.unresolved {
            let _ = writeln!(out, "  {}:{}  {}", call.file, call.line, call.expression);
        }
    }
    out
}
