//! Import statements in a Python file, read with tree-sitter.
use super::{first_argument, Facts, Import};
use crate::provider::typescript::line;
use anyhow::{anyhow, Result};
use tree_sitter::{Node, Parser};

pub fn is_python(path: &str) -> bool {
    path.ends_with(".py")
}

pub fn parse(file: &str, source: &str) -> Result<Facts> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|error| anyhow!("cannot load the Python grammar: {error}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("tree-sitter gave up parsing {file}"))?;
    let mut facts = Facts::default();
    visit(tree.root_node(), source, false, &mut facts);
    Ok(facts)
}

fn text<'s>(node: Node, source: &'s str) -> &'s str {
    &source[node.byte_range()]
}

/// A dotted name as Python reads it: `a . b` is `a.b`.
fn dotted(node: Node, source: &str) -> String {
    text(node, source).split_whitespace().collect()
}

/// `if TYPE_CHECKING:` and `if typing.TYPE_CHECKING:` hold imports that exist
/// only for type checkers.
fn type_checking(condition: Node, source: &str) -> bool {
    matches!(
        text(condition, source),
        "TYPE_CHECKING" | "typing.TYPE_CHECKING"
    )
}

fn visit(node: Node, source: &str, type_only: bool, facts: &mut Facts) {
    let push = |facts: &mut Facts, specifier: String| {
        facts.imports.push(Import {
            line: line(node),
            specifier,
            type_only,
        })
    };
    match node.kind() {
        "import_statement" => {
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                let module = match name.kind() {
                    "aliased_import" => name.child_by_field_name("name"),
                    _ => Some(name),
                };
                if let Some(module) = module {
                    push(facts, dotted(module, source));
                }
            }
            return;
        }
        // `from .x import y` keeps its dots, so it reads as relative.
        "import_from_statement" => {
            if let Some(module) = node.child_by_field_name("module_name") {
                push(facts, dotted(module, source));
            }
            return;
        }
        "future_import_statement" => return,
        "call" => {
            let function = node.child_by_field_name("function");
            let dynamic = function.is_some_and(|function| {
                matches!(
                    text(function, source),
                    "importlib.import_module" | "__import__"
                )
            });
            if dynamic {
                let argument = node
                    .child_by_field_name("arguments")
                    .and_then(first_argument);
                match argument.and_then(|argument| literal(argument, source)) {
                    Some(module) => push(facts, module),
                    None => facts
                        .unresolved
                        .push((line(node), text(node, source).into())),
                }
            }
        }
        "if_statement" => {
            let checking = node
                .child_by_field_name("condition")
                .is_some_and(|condition| type_checking(condition, source));
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                let consequence = node
                    .child_by_field_name("consequence")
                    .is_some_and(|consequence| consequence.id() == child.id());
                visit(child, source, type_only || (checking && consequence), facts);
            }
            return;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, source, type_only, facts);
    }
}

/// The value of a plain string literal; `None` for f-strings and anything else.
fn literal(node: Node, source: &str) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let mut value = String::new();
    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        match part.kind() {
            "string_start" | "string_end" => {}
            "string_content" => value.push_str(text(part, source)),
            _ => return None,
        }
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_absolute_relative_aliased_dynamic_and_type_checking_imports() {
        let facts = parse(
            "x.py",
            "from __future__ import annotations\n\
             import numpy as np, os.path\n\
             from ortools.constraint_solver import pywrapcp, routing_enums_pb2\n\
             from . import sibling\n\
             from ..core.models import Plan\n\
             from typing import TYPE_CHECKING\n\
             if TYPE_CHECKING:\n    from pandas import DataFrame\nelse:\n    import json\n\
             def load():\n    import yaml\n    return importlib.import_module('plugins.' + name), __import__('toml')\n\
             importlib.import_module(\n    # the plugin\n    'plugin'\n)\n",
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
                (2, "numpy", false),
                (2, "os.path", false),
                (3, "ortools.constraint_solver", false),
                (4, ".", false),
                (5, "..core.models", false),
                (6, "typing", false),
                (8, "pandas", true),
                (10, "json", false),
                (12, "yaml", false),
                (13, "toml", false),
                (14, "plugin", false),
            ]
        );
        assert_eq!(
            facts.unresolved,
            [(13, "importlib.import_module('plugins.' + name)".to_owned())]
        );
    }
}
