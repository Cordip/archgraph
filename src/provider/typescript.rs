//! What the CSS and HTTP extractors share: a tree-sitter parse of one
//! TypeScript/JavaScript file, its same-file constants, and strings broken
//! into characters and embedded expressions.
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use tree_sitter::{Node, Parser, Tree};

const EXTENSIONS: [&str; 8] = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

/// A TypeScript or JavaScript source file; declaration files hold no code.
pub fn is_script(path: &str) -> bool {
    EXTENSIONS.iter().any(|ext| path.ends_with(ext)) && !path.ends_with(".d.ts")
}

pub fn parse(file: &str, source: &str) -> Result<Tree> {
    let language = if file.ends_with(".ts") || file.ends_with(".mts") || file.ends_with(".cts") {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT
    } else {
        tree_sitter_typescript::LANGUAGE_TSX
    };
    let mut parser = Parser::new();
    parser
        .set_language(&language.into())
        .map_err(|error| anyhow!("cannot load the TypeScript grammar: {error}"))?;
    parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("tree-sitter gave up parsing {file}"))
}

pub fn line(node: Node) -> usize {
    node.start_position().row + 1
}

/// A string broken into characters and embedded expressions (`${...}`, or
/// the operands of `+`), each character with its line.
pub enum Segment<'t> {
    Char(char, usize),
    Expression(Node<'t>),
}

pub struct Script<'s, 't> {
    source: &'s str,
    /// Values assigned to each name anywhere in the file (`const x = ...`).
    constants: HashMap<&'s str, Vec<Node<'t>>>,
}

impl<'s, 't> Script<'s, 't> {
    pub fn new(source: &'s str, root: Node<'t>) -> Self {
        let mut script = Self {
            source,
            constants: HashMap::new(),
        };
        script.collect_constants(root);
        script
    }

    fn collect_constants(&mut self, node: Node<'t>) {
        if node.kind() == "variable_declarator" {
            if let (Some(name), Some(value)) = (
                node.child_by_field_name("name"),
                node.child_by_field_name("value"),
            ) {
                if name.kind() == "identifier" {
                    let name = self.text(name);
                    self.constants.entry(name).or_default().push(value);
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.collect_constants(child);
        }
    }

    pub fn text(&self, node: Node) -> &'s str {
        &self.source[node.byte_range()]
    }

    pub fn constants(&self, name: &str) -> Option<&[Node<'t>]> {
        self.constants.get(name).map(Vec::as_slice)
    }

    /// The content of a string literal, escapes left as written.
    pub fn string_text(&self, node: Node) -> String {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .map(|part| self.text(part))
            .collect()
    }

    /// The pieces of a string or template literal.
    pub fn segments(&self, node: Node<'t>) -> Vec<Segment<'t>> {
        let mut segments = Vec::new();
        let mut cursor = node.walk();
        for part in node.named_children(&mut cursor) {
            if part.kind() == "template_substitution" {
                if let Some(inner) = part.named_child(0) {
                    segments.push(Segment::Expression(inner));
                }
                continue;
            }
            let mut line = line(part);
            for c in self.text(part).chars() {
                segments.push(Segment::Char(c, line));
                if c == '\n' {
                    line += 1;
                }
            }
        }
        segments
    }

    /// The pieces of `a + 'b' + c`: string operands as characters, anything
    /// else as one expression.
    pub fn concatenation(&self, node: Node<'t>, segments: &mut Vec<Segment<'t>>) {
        let operator = node.child_by_field_name("operator");
        match node.kind() {
            "binary_expression" if operator.is_some_and(|op| self.text(op) == "+") => {
                for side in ["left", "right"] {
                    if let Some(side) = node.child_by_field_name(side) {
                        self.concatenation(side, segments);
                    }
                }
            }
            "string" => segments.extend(self.segments(node)),
            _ => segments.push(Segment::Expression(node)),
        }
    }
}
