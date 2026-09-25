//! Class definitions and `@import`s of one stylesheet, via lightningcss.
use anyhow::{anyhow, Result};
use lightningcss::{
    rules::CssRule,
    selector::{Component, Selector},
    stylesheet::{ParserOptions, StyleSheet},
    visit_types,
    visitor::{Visit, VisitTypes, Visitor},
};
use std::{
    convert::Infallible,
    sync::{Arc, RwLock},
};

#[derive(Debug, Default, PartialEq)]
pub struct StylesheetFacts {
    /// Class name and 1-based line of the rule defining it.
    pub classes: Vec<(String, usize)>,
    /// `@import` target and its line.
    pub imports: Vec<(String, usize)>,
    /// Parser recoveries. Each one may have dropped a rule, so none may pass
    /// silently.
    pub warnings: Vec<String>,
}

struct Collector<'a> {
    facts: &'a mut StylesheetFacts,
}

impl<'i> Visitor<'i> for Collector<'_> {
    type Error = Infallible;

    fn visit_types(&self) -> VisitTypes {
        visit_types!(RULES)
    }

    fn visit_rule(&mut self, rule: &mut CssRule<'i>) -> Result<(), Self::Error> {
        match rule {
            CssRule::Style(style) => {
                let line = style.loc.line as usize + 1;
                for selector in &style.selectors.0 {
                    classes_in(selector, &mut |name| {
                        self.facts.classes.push((name.to_owned(), line));
                    });
                }
            }
            CssRule::Import(import) => {
                self.facts
                    .imports
                    .push((import.url.to_string(), import.loc.line as usize + 1));
            }
            _ => {}
        }
        rule.visit_children(self)
    }
}

/// Every class a selector mentions, including inside `:is()`, `:not()`,
/// `:where()`, `:has()` and `:nth-child(of ...)`.
fn classes_in(selector: &Selector, found: &mut impl FnMut(&str)) {
    for component in selector.iter_raw_match_order() {
        match component {
            Component::Class(name) => found(name.as_ref()),
            Component::Negation(list)
            | Component::Is(list)
            | Component::Where(list)
            | Component::Has(list)
            | Component::Any(_, list) => {
                for inner in list.iter() {
                    classes_in(inner, found);
                }
            }
            Component::NthOf(data) => {
                for inner in data.selectors() {
                    classes_in(inner, found);
                }
            }
            Component::Slotted(inner) | Component::Host(Some(inner)) => classes_in(inner, found),
            _ => {}
        }
    }
}

pub fn parse(file: &str, source: &str) -> Result<StylesheetFacts> {
    let warnings = Arc::new(RwLock::new(Vec::new()));
    let options = ParserOptions {
        filename: file.to_owned(),
        error_recovery: true,
        warnings: Some(warnings.clone()),
        ..ParserOptions::default()
    };
    let mut sheet = StyleSheet::parse(source, options)
        .map_err(|error| anyhow!("cannot parse stylesheet {file}: {error}"))?;
    let mut facts = StylesheetFacts::default();
    sheet
        .visit(&mut Collector { facts: &mut facts })
        .unwrap_or_else(|never| match never {});
    facts.warnings = warnings
        .read()
        .map_err(|_| anyhow!("stylesheet warning list is poisoned"))?
        .iter()
        .map(|warning| format!("{file}: {warning}"))
        .collect();
    facts.classes.sort();
    facts.classes.dedup();
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_classes_in_nested_and_functional_selectors_with_lines() {
        let css = "@import './base.css';\n\
            .panel, .panel-head > h2 { color: red }\n\
            @media (width < 600px) {\n  .compact:is(.a, .b) { margin: 0 }\n}\n\
            .card { &.on { color: blue } }\n\
            li:not(.done):nth-child(2 of .item) { opacity: 1 }\n\
            #main { top: 0 }\n";
        let facts = parse("styles.css", css).unwrap();
        let names: Vec<(&str, usize)> = facts
            .classes
            .iter()
            .map(|(name, line)| (name.as_str(), *line))
            .collect();
        assert_eq!(
            names,
            [
                ("a", 4),
                ("b", 4),
                ("card", 6),
                ("compact", 4),
                ("done", 7),
                ("item", 7),
                ("on", 6),
                ("panel", 2),
                ("panel-head", 2),
            ]
        );
        assert_eq!(facts.imports, [("./base.css".to_owned(), 1)]);
        assert!(facts.warnings.is_empty(), "{:?}", facts.warnings);
    }

    #[test]
    fn recovered_errors_are_reported() {
        let facts = parse("broken.css", ".a { color: red }\n}\n.b { color: blue }\n").unwrap();
        assert!(!facts.warnings.is_empty());
        assert!(facts.warnings[0].starts_with("broken.css: "));
    }
}
