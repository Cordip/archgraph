//! Class names and stylesheet imports in TypeScript/JavaScript, via
//! tree-sitter. Class names come from `className`/`class` JSX attributes,
//! `className:` object properties (e.g. Leaflet options), `el.className =`,
//! `el.classList.add/remove/toggle/replace(...)` and `class="..."` inside
//! HTML strings. What cannot be resolved statically is recorded, not
//! guessed.
use crate::model::ClassCertainty;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use tree_sitter::{Node, Parser};

#[derive(Debug, PartialEq)]
pub struct DynamicUse {
    pub line: usize,
    pub expression: String,
    pub prefix: Option<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct ScriptFacts {
    /// Import specifiers ending in `.css`, with their lines.
    pub stylesheet_imports: Vec<(String, usize)>,
    pub classes: Vec<(String, usize, ClassCertainty)>,
    pub dynamic: Vec<DynamicUse>,
    /// Syntax errors; class names near them may be missed.
    pub warnings: Vec<String>,
}

const CLASS_PROPERTIES: [&str; 2] = ["className", "class"];
/// Constants followed through `className={name}`; deeper chains are unusual.
const MAX_CONSTANT_DEPTH: usize = 8;
const EXPRESSION_LENGTH: usize = 80;

pub fn parse(file: &str, source: &str) -> Result<ScriptFacts> {
    let language = if file.ends_with(".ts") || file.ends_with(".mts") || file.ends_with(".cts") {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT
    } else {
        tree_sitter_typescript::LANGUAGE_TSX
    };
    let mut parser = Parser::new();
    parser
        .set_language(&language.into())
        .map_err(|error| anyhow!("cannot load the TypeScript grammar: {error}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("tree-sitter gave up parsing {file}"))?;
    let mut scan = Scan {
        source,
        constants: HashMap::new(),
        facts: ScriptFacts::default(),
    };
    let root = tree.root_node();
    scan.collect_constants(root);
    scan.visit(root);
    if root.has_error() {
        scan.facts.warnings.push(format!(
            "{file}: syntax error; class names near it may be missed"
        ));
    }
    Ok(scan.facts)
}

/// A string broken into characters and embedded expressions (`${...}`, or
/// the operands of `+`), each character with its line.
enum Segment<'t> {
    Char(char, usize),
    Expression(Node<'t>),
}

struct Scan<'s, 't> {
    source: &'s str,
    constants: HashMap<&'s str, Vec<Node<'t>>>,
    facts: ScriptFacts,
}

fn line(node: Node) -> usize {
    node.start_position().row + 1
}

impl<'s, 't> Scan<'s, 't> {
    fn text(&self, node: Node) -> &'s str {
        &self.source[node.byte_range()]
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

    fn visit(&mut self, node: Node<'t>) {
        match node.kind() {
            "import_statement" => {
                if let Some(source) = node.child_by_field_name("source") {
                    let specifier = self.string_text(source);
                    if specifier.ends_with(".css") {
                        self.facts
                            .stylesheet_imports
                            .push((specifier, line(source)));
                    }
                }
            }
            "jsx_attribute" => {
                let mut cursor = node.walk();
                let children: Vec<Node> = node.named_children(&mut cursor).collect();
                if let [name, value] = children[..] {
                    if CLASS_PROPERTIES.contains(&self.text(name)) {
                        self.class_value(value, ClassCertainty::Literal, 0);
                    }
                }
            }
            "pair" => {
                if let (Some(key), Some(value)) = (
                    node.child_by_field_name("key"),
                    node.child_by_field_name("value"),
                ) {
                    let key = match key.kind() {
                        "string" => self.string_text(key),
                        _ => self.text(key).to_owned(),
                    };
                    if CLASS_PROPERTIES.contains(&key.as_str()) {
                        self.class_value(value, ClassCertainty::Literal, 0);
                    }
                }
            }
            "assignment_expression" => {
                let left = node.child_by_field_name("left");
                let target = left
                    .filter(|left| left.kind() == "member_expression")
                    .and_then(|left| left.child_by_field_name("property"));
                if target.is_some_and(|property| CLASS_PROPERTIES.contains(&self.text(property))) {
                    if let Some(right) = node.child_by_field_name("right") {
                        self.class_value(right, ClassCertainty::Literal, 0);
                    }
                }
            }
            "call_expression" => self.class_list_call(node),
            "string" | "template_string" => self.markup(node),
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.visit(child);
        }
    }

    /// The content of a string literal, escapes left as written.
    fn string_text(&self, node: Node) -> String {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .map(|part| self.text(part))
            .collect()
    }

    /// `certainty` applies to plain strings; anything nested in an
    /// expression is at most `Expression`.
    fn class_value(&mut self, node: Node<'t>, certainty: ClassCertainty, depth: usize) {
        let nested = ClassCertainty::Expression;
        match node.kind() {
            "string" => {
                for (name, line) in words(&self.segments(node)) {
                    self.facts.classes.push((name, line, certainty));
                }
            }
            "jsx_expression" | "parenthesized_expression" => {
                if let Some(inner) = node.named_child(0) {
                    let certainty = if node.kind() == "jsx_expression" {
                        certainty
                    } else {
                        nested
                    };
                    self.class_value(inner, certainty, depth);
                }
            }
            "template_string" => {
                let segments = self.segments(node);
                self.tokens(&segments, node, nested, depth);
            }
            "ternary_expression" => {
                for field in ["consequence", "alternative"] {
                    if let Some(branch) = node.child_by_field_name(field) {
                        self.class_value(branch, nested, depth);
                    }
                }
            }
            "binary_expression" => {
                let operator = node
                    .child_by_field_name("operator")
                    .map(|operator| self.text(operator));
                let (Some(left), Some(right)) = (
                    node.child_by_field_name("left"),
                    node.child_by_field_name("right"),
                ) else {
                    return;
                };
                match operator {
                    // `cond && 'x'`: the left side is the condition.
                    Some("&&") => self.class_value(right, nested, depth),
                    Some("||" | "??") => {
                        self.class_value(left, nested, depth);
                        self.class_value(right, nested, depth);
                    }
                    Some("+") => {
                        let mut segments = Vec::new();
                        self.concatenation(node, &mut segments);
                        self.tokens(&segments, node, nested, depth);
                    }
                    _ => self.dynamic(node, None),
                }
            }
            "array" => {
                let mut cursor = node.walk();
                for element in node.named_children(&mut cursor) {
                    self.class_value(element, nested, depth);
                }
            }
            "spread_element" => {
                if let Some(inner) = node.named_child(0) {
                    self.class_value(inner, nested, depth);
                }
            }
            "call_expression" => self.call(node, depth),
            "identifier" => {
                let values = self.constants.get(self.text(node)).cloned();
                match values {
                    Some(values) if depth < MAX_CONSTANT_DEPTH => {
                        for value in values {
                            self.class_value(value, nested, depth + 1);
                        }
                    }
                    _ => self.dynamic(node, None),
                }
            }
            // A missing class, e.g. `cond && 'x'` when `cond` is false.
            "null" | "undefined" | "false" | "true" => {}
            _ => self.dynamic(node, None),
        }
    }

    /// `el.classList.toggle('x', on)` and friends; `toggle` takes one class,
    /// the others take all their arguments.
    fn class_list_call(&mut self, node: Node<'t>) {
        let Some(function) = node
            .child_by_field_name("function")
            .filter(|function| function.kind() == "member_expression")
        else {
            return;
        };
        let method = function
            .child_by_field_name("property")
            .map(|p| self.text(p));
        let on_class_list = function
            .child_by_field_name("object")
            .filter(|object| object.kind() == "member_expression")
            .and_then(|object| object.child_by_field_name("property"))
            .is_some_and(|property| self.text(property) == "classList");
        let taken = match method {
            Some("toggle") => 1,
            Some("add" | "remove" | "replace") => usize::MAX,
            _ => return,
        };
        if !on_class_list {
            return;
        }
        if let Some(arguments) = node.child_by_field_name("arguments") {
            let mut cursor = arguments.walk();
            let arguments: Vec<Node> = arguments.named_children(&mut cursor).take(taken).collect();
            for argument in arguments {
                self.class_value(argument, ClassCertainty::Expression, 0);
            }
        }
    }

    /// `[...].join(' ')` joins its elements; any other call (`clsx`,
    /// `classnames`, ...) is read as a class helper: string arguments and
    /// the keys of object arguments.
    fn call(&mut self, node: Node<'t>, depth: usize) {
        let nested = ClassCertainty::Expression;
        let function = node.child_by_field_name("function");
        if let Some(function) = function.filter(|f| f.kind() == "member_expression") {
            let property = function.child_by_field_name("property");
            if property.is_some_and(|property| self.text(property) == "join") {
                if let Some(object) = function.child_by_field_name("object") {
                    self.class_value(object, nested, depth);
                }
                return;
            }
        }
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return;
        };
        let mut cursor = arguments.walk();
        for argument in arguments.named_children(&mut cursor) {
            if argument.kind() == "object" {
                let mut pairs = argument.walk();
                for pair in argument.named_children(&mut pairs) {
                    match pair.child_by_field_name("key") {
                        Some(key) if key.kind() == "property_identifier" => self
                            .facts
                            .classes
                            .push((self.text(key).to_owned(), line(key), nested)),
                        Some(key) if key.kind() == "string" => {
                            for (name, line) in words(&self.segments(key)) {
                                self.facts.classes.push((name, line, nested));
                            }
                        }
                        _ => self.dynamic(pair, None),
                    }
                }
            } else {
                self.class_value(argument, nested, depth);
            }
        }
    }

    fn dynamic(&mut self, node: Node, prefix: Option<String>) {
        let expression: String = self
            .text(node)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(EXPRESSION_LENGTH)
            .collect();
        self.facts.dynamic.push(DynamicUse {
            line: line(node),
            expression,
            prefix,
        });
    }

    fn segments(&self, node: Node<'t>) -> Vec<Segment<'t>> {
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

    fn concatenation(&self, node: Node<'t>, segments: &mut Vec<Segment<'t>>) {
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

    /// Splits a class string with embedded expressions into class names. A
    /// word that is exactly one expression is resolved like any class value;
    /// a word mixing text and expressions is dynamic, with its known prefix.
    fn tokens(
        &mut self,
        segments: &[Segment<'t>],
        whole: Node,
        certainty: ClassCertainty,
        depth: usize,
    ) {
        for word in
            segments.split(|segment| matches!(segment, Segment::Char(c, _) if c.is_whitespace()))
        {
            match word {
                [] => {}
                [Segment::Expression(expression)] => {
                    self.class_value(*expression, ClassCertainty::Expression, depth)
                }
                _ if word.iter().all(|s| matches!(s, Segment::Char(..))) => {
                    let (name, line) = static_word(word);
                    self.facts.classes.push((name, line, certainty));
                }
                _ => {
                    let prefix: String = word
                        .iter()
                        .map_while(|segment| match segment {
                            Segment::Char(c, _) => Some(*c),
                            Segment::Expression(_) => None,
                        })
                        .collect();
                    self.dynamic(whole, Some(prefix).filter(|p| !p.is_empty()));
                }
            }
        }
    }

    /// `class="..."` attributes inside an HTML string.
    fn markup(&mut self, node: Node<'t>) {
        let segments = self.segments(node);
        let chars: Vec<Option<char>> = segments
            .iter()
            .map(|segment| match segment {
                Segment::Char(c, _) => Some(*c),
                Segment::Expression(_) => None,
            })
            .collect();
        let mut index = 0;
        while let Some(start) = find_class_attribute(&chars, index) {
            let quote = chars[start];
            let Some(length) = chars[start + 1..].iter().position(|c| *c == quote) else {
                break;
            };
            let value = &segments[start + 1..start + 1 + length];
            self.tokens(value, node, ClassCertainty::Markup, 0);
            index = start + 1 + length;
        }
    }
}

/// Index of the opening quote of the next `class="` or `class='` at or
/// after `from`.
fn find_class_attribute(chars: &[Option<char>], from: usize) -> Option<usize> {
    const NAME: [char; 5] = ['c', 'l', 'a', 's', 's'];
    let mut index = from;
    while index + NAME.len() < chars.len() {
        let preceded_by_space =
            index == 0 || chars[index - 1].is_some_and(|c| c.is_whitespace() || c == '<');
        let name_matches = NAME
            .iter()
            .enumerate()
            .all(|(offset, c)| chars[index + offset] == Some(*c));
        if preceded_by_space && name_matches {
            let mut cursor = index + NAME.len();
            let skip = |cursor: &mut usize| {
                while chars
                    .get(*cursor)
                    .is_some_and(|c| c.is_some_and(char::is_whitespace))
                {
                    *cursor += 1;
                }
            };
            skip(&mut cursor);
            if chars.get(cursor) == Some(&Some('=')) {
                cursor += 1;
                skip(&mut cursor);
                if matches!(chars.get(cursor), Some(Some('"' | '\''))) {
                    return Some(cursor);
                }
            }
        }
        index += 1;
    }
    None
}

fn static_word(word: &[Segment]) -> (String, usize) {
    let mut line = 0;
    let name = word
        .iter()
        .filter_map(|segment| match segment {
            Segment::Char(c, at) => {
                line = *at;
                Some(*c)
            }
            Segment::Expression(_) => None,
        })
        .collect();
    (name, line)
}

/// The whitespace-separated words of an expression-free string.
fn words(segments: &[Segment]) -> Vec<(String, usize)> {
    segments
        .split(|segment| matches!(segment, Segment::Char(c, _) if c.is_whitespace()))
        .filter(|word| !word.is_empty())
        .map(static_word)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ClassCertainty::{Expression, Literal, Markup};

    fn classes(facts: &ScriptFacts) -> Vec<(&str, usize, ClassCertainty)> {
        facts
            .classes
            .iter()
            .map(|(name, line, certainty)| (name.as_str(), *line, *certainty))
            .collect()
    }

    #[test]
    fn jsx_attributes_expressions_and_constants() {
        let source = r#"import './styles.css';
import 'leaflet/dist/leaflet.css';
import { x } from './x';
const cls = ['vm', ok ? 'good' : 'bad'].join(' ');
export const A = () => (
  <div className="panel engineers">
    <i className={`tier-seg t${t.tier} ${on ? 'on' : ''}`} />
    <b className={warn && 'warn'} class='q' />
    <span className={cls} />
    <em className={props.kind} />
    <u className={'a ' + extra + ' b'} />
  </div>
);
"#;
        let facts = parse("A.tsx", source).unwrap();
        assert_eq!(
            facts.stylesheet_imports,
            [
                ("./styles.css".to_owned(), 1),
                ("leaflet/dist/leaflet.css".to_owned(), 2)
            ]
        );
        let mut found = classes(&facts);
        found.sort();
        assert_eq!(
            found,
            [
                ("a", 11, Expression),
                ("b", 11, Expression),
                ("bad", 4, Expression),
                ("engineers", 6, Literal),
                ("good", 4, Expression),
                ("on", 7, Expression),
                ("panel", 6, Literal),
                ("q", 8, Literal),
                ("tier-seg", 7, Expression),
                ("vm", 4, Expression),
                ("warn", 8, Expression),
            ]
        );
        let dynamic: Vec<(&str, Option<&str>)> = facts
            .dynamic
            .iter()
            .map(|d| (d.expression.as_str(), d.prefix.as_deref()))
            .collect();
        assert_eq!(
            dynamic,
            [
                ("`tier-seg t${t.tier} ${on ? 'on' : ''}`", Some("t")),
                ("props.kind", None),
                ("extra", None),
            ]
        );
        assert!(facts.warnings.is_empty());
    }

    #[test]
    fn object_properties_and_html_strings() {
        let source = "const icon = L.divIcon({\n  className: 'vm-wrap',\n  html: `<div class=\"em em-${status}\" style=\"--c:${c}\">${n}</div>`,\n});\nconst plain = '<span class=\"office-mark hollow\"></span>';\nconst notMarkup = 'subclass=\"x\"';\n";
        let facts = parse("icons.ts", source).unwrap();
        let mut found = classes(&facts);
        found.sort();
        assert_eq!(
            found,
            [
                ("em", 3, Markup),
                ("hollow", 5, Markup),
                ("office-mark", 5, Markup),
                ("vm-wrap", 2, Literal),
            ]
        );
        assert_eq!(facts.dynamic.len(), 1);
        assert_eq!(facts.dynamic[0].prefix.as_deref(), Some("em-"));
        assert_eq!(facts.dynamic[0].line, 3);
    }

    #[test]
    fn dom_class_apis() {
        let source = "el.classList.toggle('picking', Boolean(on));
el.classList.add('a', 'b');
node.className = 'wrap';
list.add('not-a-class');
";
        let facts = parse("dom.ts", source).unwrap();
        let mut found = classes(&facts);
        found.sort();
        assert_eq!(
            found,
            [
                ("a", 2, Expression),
                ("b", 2, Expression),
                ("picking", 1, Expression),
                ("wrap", 3, Literal),
            ]
        );
    }

    #[test]
    fn helper_calls_and_syntax_errors() {
        let source = "const a = <p className={clsx('base', { active: on, 'is-open': open }, cond && 'x')} />;\nconst b = <p className={\n";
        let facts = parse("B.tsx", source).unwrap();
        let mut names: Vec<&str> = facts.classes.iter().map(|(n, ..)| n.as_str()).collect();
        names.sort();
        assert_eq!(names, ["active", "base", "is-open", "x"]);
        assert_eq!(facts.warnings.len(), 1);
    }
}
