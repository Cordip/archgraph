//! HTTP calls in TypeScript/JavaScript, via tree-sitter: `fetch`,
//! `new EventSource`, `new WebSocket`, `axios.<verb>` and
//! `navigator.sendBeacon`, plus functions of the same file that pass one of
//! their parameters into such a call (`request('/plans')` for
//! `function request(path) { return fetch(BASE + path) }`). URLs are
//! evaluated through the file's string constants; any other value becomes a
//! path parameter. A string built on the same constant as a call
//! (`${BASE}/plans/${id}/export`, used as a link) is a reference too.
use crate::provider::typescript::{self, line, Script, Segment};
use anyhow::Result;
use std::collections::{BTreeSet, HashMap};
use tree_sitter::Node;

#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String),
    /// A value unknown statically: an id, a variable, a call.
    Param,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Via {
    /// `fetch(url)` and the other client APIs.
    Call,
    /// A call of a same-file function that passes its parameter to one.
    Wrapper,
    /// A string built on the base constant of a call, e.g. a download link.
    Link,
}

#[derive(Debug, PartialEq)]
pub struct Call {
    pub line: usize,
    /// Upper case; `None` when it cannot be told statically.
    pub method: Option<String>,
    pub url: Vec<Piece>,
    pub via: Via,
}

#[derive(Debug, PartialEq)]
pub struct Unresolved {
    pub line: usize,
    pub expression: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct ClientFacts {
    pub calls: Vec<Call>,
    /// Calls whose URL does not start with a known string.
    pub unresolved: Vec<Unresolved>,
    pub warnings: Vec<String>,
}

const AXIOS_VERBS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];
const FUNCTIONS: [&str; 3] = [
    "function_declaration",
    "arrow_function",
    "function_expression",
];
/// Constants followed through a URL; deeper chains are unusual.
const MAX_DEPTH: usize = 8;
const EXPRESSION_LENGTH: usize = 80;

pub fn parse(file: &str, source: &str) -> Result<ClientFacts> {
    let tree = typescript::parse(file, source)?;
    let root = tree.root_node();
    let mut scan = Scan {
        script: Script::new(source, root),
        facts: ClientFacts::default(),
        wrappers: Vec::new(),
        consumed: Vec::new(),
        bases: BTreeSet::new(),
    };
    scan.find_calls(root);
    scan.wrapper_calls(root);
    scan.links(root);
    if root.has_error() {
        scan.facts.warnings.push(format!(
            "{file}: syntax error; HTTP calls near it may be missed"
        ));
    }
    scan.facts.calls.sort_by_key(|call| call.line);
    scan.facts
        .unresolved
        .sort_by_key(|unresolved| unresolved.line);
    Ok(scan.facts)
}

/// How a call's HTTP method is known.
#[derive(Clone)]
enum Method {
    Known(Option<String>),
    /// Taken from this parameter's argument (`fetch(url, init)` in a wrapper).
    FromParameter(String),
}

struct Wrapper<'t> {
    name: String,
    parameters: Vec<String>,
    function: Node<'t>,
    url: Node<'t>,
    method: Method,
}

/// What a URL expression may refer to: the parameters of the function
/// around it, and the arguments bound to a wrapper's parameters.
#[derive(Default)]
struct Scope<'a, 't> {
    parameters: &'a [String],
    bindings: HashMap<&'a str, Node<'t>>,
}

struct Scan<'s, 't> {
    script: Script<'s, 't>,
    facts: ClientFacts,
    wrappers: Vec<Wrapper<'t>>,
    /// URL expressions already read as part of a call.
    consumed: Vec<std::ops::Range<usize>>,
    /// Constants that start the URL of a call (`BASE` in `BASE + path`).
    bases: BTreeSet<&'s str>,
}

impl<'s, 't> Scan<'s, 't> {
    fn text(&self, node: Node) -> &'s str {
        self.script.text(node)
    }

    /// The function a call calls. tree-sitter reads `await f<T>(x)` as
    /// `(await f)<T>(x)`, so the `await` is looked through.
    fn callee(node: Node<'t>) -> Option<Node<'t>> {
        let function = node.child_by_field_name("function")?;
        if function.kind() == "await_expression" {
            return function.named_child(0);
        }
        Some(function)
    }

    fn arguments(node: Node<'t>) -> Vec<Node<'t>> {
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return Vec::new();
        };
        let mut cursor = arguments.walk();
        arguments.named_children(&mut cursor).collect()
    }

    /// The URL argument and method of a client API call, if `node` is one.
    fn client_call(&self, node: Node<'t>) -> Option<(Node<'t>, Method)> {
        let arguments = Self::arguments(node);
        let url = *arguments.first()?;
        let get = || Method::Known(Some("GET".into()));
        match node.kind() {
            "new_expression" => match self.text(node.child_by_field_name("constructor")?) {
                "EventSource" => Some((url, get())),
                "WebSocket" => Some((url, Method::Known(None))),
                _ => None,
            },
            "call_expression" => {
                let function = Self::callee(node)?;
                let (object, name) = match function.kind() {
                    "identifier" => (None, self.text(function)),
                    "member_expression" => (
                        Some(self.text(function.child_by_field_name("object")?)),
                        self.text(function.child_by_field_name("property")?),
                    ),
                    _ => return None,
                };
                match (object, name) {
                    (None | Some("window" | "globalThis"), "fetch") => {
                        Some((url, self.init_method(arguments.get(1).copied())))
                    }
                    (Some("axios"), verb) if AXIOS_VERBS.contains(&verb) => {
                        Some((url, Method::Known(Some(verb.to_uppercase()))))
                    }
                    (Some("navigator"), "sendBeacon") => {
                        Some((url, Method::Known(Some("POST".into()))))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The method of `fetch(url, init)`: GET without `init` or without a
    /// `method` in it.
    fn init_method(&self, init: Option<Node<'t>>) -> Method {
        let Some(init) = init else {
            return Method::Known(Some("GET".into()));
        };
        match init.kind() {
            "object" => {
                let mut forwarded = None;
                let mut cursor = init.walk();
                for member in init.named_children(&mut cursor) {
                    match member.kind() {
                        "pair" => {
                            let key = member.child_by_field_name("key").map(|k| self.text(k));
                            if matches!(key, Some("method" | "'method'" | "\"method\"")) {
                                let value = member.child_by_field_name("value");
                                return Method::Known(
                                    value
                                        .filter(|value| value.kind() == "string")
                                        .map(|value| self.script.string_text(value).to_uppercase()),
                                );
                            }
                        }
                        "spread_element" => {
                            forwarded = member
                                .named_child(0)
                                .filter(|inner| inner.kind() == "identifier")
                                .map(|inner| self.text(inner).to_owned());
                        }
                        _ => {}
                    }
                }
                match forwarded {
                    Some(parameter) => Method::FromParameter(parameter),
                    None => Method::Known(Some("GET".into())),
                }
            }
            "identifier" => Method::FromParameter(self.text(init).to_owned()),
            _ => Method::Known(None),
        }
    }

    /// The nearest function around `node`, with its name when it has one
    /// that can be called (`function f`, `const f = () => ...`).
    fn enclosing_function(node: Node<'t>) -> Option<Node<'t>> {
        let mut current = node.parent();
        while let Some(parent) = current {
            if FUNCTIONS.contains(&parent.kind()) {
                return Some(parent);
            }
            current = parent.parent();
        }
        None
    }

    fn function_name(&self, function: Node<'t>) -> Option<String> {
        if let Some(name) = function.child_by_field_name("name") {
            return Some(self.text(name).to_owned());
        }
        let declarator = function
            .parent()
            .filter(|parent| parent.kind() == "variable_declarator")?;
        Some(
            self.text(declarator.child_by_field_name("name")?)
                .to_owned(),
        )
    }

    fn parameters(&self, function: Node<'t>) -> Vec<String> {
        if let Some(single) = function.child_by_field_name("parameter") {
            return vec![self.text(single).to_owned()];
        }
        let Some(list) = function.child_by_field_name("parameters") else {
            return Vec::new();
        };
        let mut cursor = list.walk();
        list.named_children(&mut cursor)
            .map(|parameter| {
                parameter
                    .child_by_field_name("pattern")
                    .map_or_else(String::new, |pattern| self.text(pattern).to_owned())
            })
            .collect()
    }

    fn find_calls(&mut self, node: Node<'t>) {
        if let Some((url, method)) = self.client_call(node) {
            self.consumed.push(url.byte_range());
            let function = Self::enclosing_function(node);
            let parameters = function.map_or_else(Vec::new, |f| self.parameters(f));
            let scope = Scope {
                parameters: &parameters,
                ..Scope::default()
            };
            let mut touched = false;
            let pieces = self.evaluate(url, &scope, 0, &mut touched);
            let from_parameter =
                matches!(&method, Method::FromParameter(p) if parameters.contains(p));
            let name = function.and_then(|f| self.function_name(f));
            match (function, name) {
                (Some(function), Some(name)) if touched || from_parameter => {
                    self.wrappers.push(Wrapper {
                        name,
                        parameters,
                        function,
                        url,
                        method,
                    })
                }
                _ => {
                    let method = match method {
                        Method::Known(method) => method,
                        Method::FromParameter(_) => None,
                    };
                    self.record(url, pieces, method, Via::Call);
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.find_calls(child);
        }
    }

    fn record(&mut self, url: Node<'t>, pieces: Vec<Piece>, method: Option<String>, via: Via) {
        if matches!(pieces.first(), Some(Piece::Text(_))) {
            if let Some(base) = self.leading_constant(url) {
                self.bases.insert(base);
            }
            self.facts.calls.push(Call {
                line: line(url),
                method,
                url: pieces,
                via,
            });
        } else {
            self.unresolved(url);
        }
    }

    fn unresolved(&mut self, node: Node) {
        let expression: String = self
            .text(node)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(EXPRESSION_LENGTH)
            .collect();
        self.facts.unresolved.push(Unresolved {
            line: line(node),
            expression,
        });
    }

    /// `BASE` in `BASE + path` or `` `${BASE}/x` ``, when it is a constant.
    fn leading_constant(&self, url: Node<'t>) -> Option<&'s str> {
        let first = match url.kind() {
            "template_string" => match self.script.segments(url).into_iter().next()? {
                Segment::Expression(expression) => expression,
                Segment::Char(..) => return None,
            },
            "binary_expression" => {
                let mut segments = Vec::new();
                self.script.concatenation(url, &mut segments);
                match segments.into_iter().next()? {
                    Segment::Expression(expression) => expression,
                    Segment::Char(..) => return None,
                }
            }
            _ => url,
        };
        let name = self.text(first);
        (first.kind() == "identifier" && self.script.constants(name).is_some()).then_some(name)
    }

    /// Calls of the wrappers found in this file.
    fn wrapper_calls(&mut self, node: Node<'t>) {
        let mut called = vec![false; self.wrappers.len()];
        self.visit_wrapper_calls(node, &mut called);
        let uncalled: Vec<Node<'t>> = self
            .wrappers
            .iter()
            .zip(called)
            .filter(|(_, called)| !called)
            .map(|(wrapper, _)| wrapper.url)
            .collect();
        // Called only from other files: the URL is not known here.
        for url in uncalled {
            self.unresolved(url);
        }
    }

    fn visit_wrapper_calls(&mut self, node: Node<'t>, called: &mut [bool]) {
        if node.kind() == "call_expression" {
            let function = Self::callee(node);
            let name = function
                .filter(|function| function.kind() == "identifier")
                .map(|function| self.text(function));
            let wrapper = name.and_then(|name| {
                self.wrappers.iter().position(|wrapper| {
                    wrapper.name == name
                        && !wrapper.function.byte_range().contains(&node.start_byte())
                })
            });
            if let Some(index) = wrapper {
                called[index] = true;
                let arguments = Self::arguments(node);
                let wrapper = &self.wrappers[index];
                let bindings: HashMap<&str, Node<'t>> = wrapper
                    .parameters
                    .iter()
                    .map(String::as_str)
                    .zip(arguments.iter().copied())
                    .collect();
                let method = match &wrapper.method {
                    Method::Known(method) => method.clone(),
                    Method::FromParameter(parameter) => {
                        let argument = bindings.get(parameter.as_str()).copied();
                        match self.init_method(argument) {
                            Method::Known(method) => method,
                            Method::FromParameter(_) => None,
                        }
                    }
                };
                let (url, parameters) = (wrapper.url, wrapper.parameters.clone());
                let scope = Scope {
                    parameters: &parameters,
                    bindings,
                };
                let pieces = self.evaluate(url, &scope, 0, &mut false);
                if matches!(pieces.first(), Some(Piece::Text(_))) {
                    self.facts.calls.push(Call {
                        line: line(node),
                        method,
                        url: pieces,
                        via: Via::Wrapper,
                    });
                } else {
                    self.unresolved(node);
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.visit_wrapper_calls(child, called);
        }
    }

    /// Strings built on a call's base constant, outside any call.
    fn links(&mut self, node: Node<'t>) {
        let is_concatenation = node.kind() == "binary_expression"
            && node
                .child_by_field_name("operator")
                .is_some_and(|op| self.text(op) == "+");
        let inside_call = self
            .consumed
            .iter()
            .any(|range| range.contains(&node.start_byte()));
        if (node.kind() == "template_string" || is_concatenation) && !inside_call {
            if let Some(base) = self.leading_constant(node) {
                if self.bases.contains(base) {
                    let pieces = self.evaluate(node, &Scope::default(), 0, &mut false);
                    self.facts.calls.push(Call {
                        line: line(node),
                        method: None,
                        url: pieces,
                        via: Via::Link,
                    });
                    // Its operands are part of this string, not strings of their own.
                    return;
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.links(child);
        }
    }

    /// The pieces of a URL expression. `touched` reports whether it uses a
    /// parameter of the function around it.
    fn evaluate(
        &self,
        node: Node<'t>,
        scope: &Scope<'_, 't>,
        depth: usize,
        touched: &mut bool,
    ) -> Vec<Piece> {
        let mut pieces = Vec::new();
        self.evaluate_into(node, scope, depth, touched, &mut pieces);
        let mut merged: Vec<Piece> = Vec::new();
        for piece in pieces {
            match (merged.last_mut(), piece) {
                (Some(Piece::Text(last)), Piece::Text(text)) => last.push_str(&text),
                (_, piece) => merged.push(piece),
            }
        }
        merged
    }

    fn evaluate_into(
        &self,
        node: Node<'t>,
        scope: &Scope<'_, 't>,
        depth: usize,
        touched: &mut bool,
        pieces: &mut Vec<Piece>,
    ) {
        let segments = match node.kind() {
            "string" => {
                pieces.push(Piece::Text(self.script.string_text(node)));
                return;
            }
            "template_string" => self.script.segments(node),
            "binary_expression" => {
                let mut segments = Vec::new();
                self.script.concatenation(node, &mut segments);
                if matches!(&segments[..], [Segment::Expression(only)] if *only == node) {
                    pieces.push(Piece::Param);
                    return;
                }
                segments
            }
            "parenthesized_expression"
            | "as_expression"
            | "non_null_expression"
            | "satisfies_expression" => {
                if let Some(inner) = node.named_child(0) {
                    self.evaluate_into(inner, scope, depth, touched, pieces);
                }
                return;
            }
            "identifier" => {
                let name = self.text(node);
                if let Some(argument) = scope.bindings.get(name) {
                    self.evaluate_into(*argument, &Scope::default(), depth, touched, pieces);
                } else if scope.parameters.iter().any(|parameter| parameter == name) {
                    *touched = true;
                    pieces.push(Piece::Param);
                } else {
                    match self.script.constants(name) {
                        Some([value]) if depth < MAX_DEPTH => {
                            self.evaluate_into(*value, scope, depth + 1, touched, pieces)
                        }
                        _ => pieces.push(Piece::Param),
                    }
                }
                return;
            }
            _ => {
                pieces.push(Piece::Param);
                return;
            }
        };
        for segment in segments {
            match segment {
                Segment::Char(c, _) => pieces.push(Piece::Text(c.to_string())),
                Segment::Expression(expression) => {
                    self.evaluate_into(expression, scope, depth, touched, pieces)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(pieces: &[Piece]) -> String {
        pieces
            .iter()
            .map(|piece| match piece {
                Piece::Text(text) => text.as_str(),
                Piece::Param => "{}",
            })
            .collect()
    }

    fn calls(facts: &ClientFacts) -> Vec<(usize, Option<&str>, String, Via)> {
        facts
            .calls
            .iter()
            .map(|call| (call.line, call.method.as_deref(), url(&call.url), call.via))
            .collect()
    }

    #[test]
    fn wrappers_bases_and_links_resolve_to_urls() {
        let source = r#"const BASE = '/api'
async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(BASE + path, { ...init, headers: {} })
  return response.json()
}
export const api = {
  scenarios: () => request<string[]>('/scenarios'),
  candidates: async (id: string) => await request<A[] | { b: A[] }>(`/plans/${id}/candidates`),
  select: (id: string) => request(`/plans/${id}/select`, { method: 'POST' }),
  upload: (form: FormData) => fetch(`${BASE}/scenarios/upload`, { method: 'post', body: form }),
  events: (id: string) => new EventSource(`${BASE}/plans/${encodeURIComponent(id)}/events`),
  exportUrl: (id: string) => `${BASE}/plans/${id}/export`,
  label: `${prefix}-x`,
}
"#;
        let facts = parse("api.ts", source).unwrap();
        assert_eq!(
            calls(&facts),
            [
                (7, Some("GET"), "/api/scenarios".into(), Via::Wrapper),
                (
                    8,
                    Some("GET"),
                    "/api/plans/{}/candidates".into(),
                    Via::Wrapper
                ),
                (9, Some("POST"), "/api/plans/{}/select".into(), Via::Wrapper),
                (10, Some("POST"), "/api/scenarios/upload".into(), Via::Call),
                (11, Some("GET"), "/api/plans/{}/events".into(), Via::Call),
                (12, None, "/api/plans/{}/export".into(), Via::Link),
            ]
        );
        assert!(facts.unresolved.is_empty(), "{:?}", facts.unresolved);
        assert!(facts.warnings.is_empty());
    }

    #[test]
    fn client_apis_and_unknown_urls() {
        let source = r#"axios.delete('/items/' + id)
navigator.sendBeacon('/log', data)
new WebSocket('/ws')
fetch(props.url)
const get = (path) => fetch(path)
window.fetch('/plain', options)
"#;
        let facts = parse("client.js", source).unwrap();
        assert_eq!(
            calls(&facts),
            [
                (1, Some("DELETE"), "/items/{}".into(), Via::Call),
                (2, Some("POST"), "/log".into(), Via::Call),
                (3, None, "/ws".into(), Via::Call),
                (6, None, "/plain".into(), Via::Call),
            ]
        );
        let unresolved: Vec<(usize, &str)> = facts
            .unresolved
            .iter()
            .map(|u| (u.line, u.expression.as_str()))
            .collect();
        // `get` is never called in this file, so its URL is unknown here.
        assert_eq!(unresolved, [(4, "props.url"), (5, "path")]);
    }
}
