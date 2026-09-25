//! Module specifiers a TypeScript/JavaScript file imports: `import`,
//! `export … from`, `import x = require()`, `import()` and `require()`.
use super::{first_argument, Facts, Import};
use crate::provider::typescript::{self, line};
use anyhow::Result;
use tree_sitter::Node;

pub fn parse(file: &str, source: &str) -> Result<Facts> {
    let tree = typescript::parse(file, source)?;
    let mut facts = Facts::default();
    visit(tree.root_node(), source, &mut facts);
    Ok(facts)
}

fn text<'s>(node: Node, source: &'s str) -> &'s str {
    &source[node.byte_range()]
}

/// `import type …` and `export type … from`; `import { type A }` still loads
/// the module under `verbatimModuleSyntax`, so it counts as a runtime import.
fn type_keyword(node: Node) -> bool {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|child| !child.is_named() && child.kind() == "type");
    found
}

/// A string, or a template string without substitutions.
fn literal(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "string" | "template_string" => {
            let mut value = String::new();
            let mut cursor = node.walk();
            for part in node.named_children(&mut cursor) {
                match part.kind() {
                    "string_fragment" | "escape_sequence" => value.push_str(text(part, source)),
                    _ => return None,
                }
            }
            Some(value)
        }
        _ => None,
    }
}

fn visit(node: Node, source: &str, facts: &mut Facts) {
    match node.kind() {
        "import_statement" | "export_statement" => {
            let require = if node.kind() == "import_statement" {
                let mut cursor = node.walk();
                let clause = node
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "import_require_clause");
                clause
            } else {
                None
            };
            let source_node = require
                .unwrap_or(node)
                .child_by_field_name("source")
                .and_then(|specifier| literal(specifier, source));
            if let Some(specifier) = source_node {
                facts.imports.push(Import {
                    line: line(node),
                    specifier,
                    type_only: type_keyword(node),
                });
            }
        }
        "call_expression" => {
            let function = node.child_by_field_name("function");
            let loads = function.is_some_and(|function| {
                function.kind() == "import"
                    || (function.kind() == "identifier" && text(function, source) == "require")
            });
            if loads {
                let argument = node
                    .child_by_field_name("arguments")
                    .and_then(first_argument);
                match argument.and_then(|argument| literal(argument, source)) {
                    Some(specifier) => facts.imports.push(Import {
                        line: line(node),
                        specifier,
                        type_only: false,
                    }),
                    None => facts
                        .unresolved
                        .push((line(node), text(node, source).into())),
                }
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, source, facts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_static_type_only_re_exported_dynamic_and_required_specifiers() {
        let facts = parse(
            "x.tsx",
            "import React, { useState } from 'react'\n\
             import type { Feature } from 'geojson'\n\
             import { type LatLon, orderPoint } from '../geo'\n\
             import 'leaflet/dist/leaflet.css'\n\
             export * from '@tanstack/react-query'\n\
             export type { Props } from './props'\n\
             const lazy = await import('chart.js')\n\
             const fs = require(`fs`)\n\
             import legacy = require('legacy-lib')\n\
             const plugin = import(name)\n\
             require.resolve('not-a-load')\n\
             const view = import(\n  // a comment first\n  '#mobile/View.vue'\n)\n",
        )
        .unwrap();
        let imports: Vec<(usize, &str, bool)> = facts
            .imports
            .iter()
            .map(|import| (import.line, import.specifier.as_str(), import.type_only))
            .collect();
        assert_eq!(
            imports,
            [
                (1, "react", false),
                (2, "geojson", true),
                (3, "../geo", false),
                (4, "leaflet/dist/leaflet.css", false),
                (5, "@tanstack/react-query", false),
                (6, "./props", true),
                (7, "chart.js", false),
                (8, "fs", false),
                (9, "legacy-lib", false),
                (12, "#mobile/View.vue", false),
            ]
        );
        assert_eq!(facts.unresolved, [(10, "import(name)".to_owned())]);
    }
}
