pub mod diff;
pub mod http;
pub mod json;
pub mod mermaid;
pub mod packages;
pub mod styles;
pub mod text;
pub mod unused;

use crate::{config::is_within, model::ArchitectureIr};
use std::collections::HashMap;

/// The architecture node owning each file, for reports filtered to a subtree.
pub(crate) struct Owners<'a>(HashMap<&'a str, &'a str>);

impl<'a> Owners<'a> {
    pub(crate) fn new(ir: &'a ArchitectureIr) -> Self {
        Self(
            ir.files
                .iter()
                .filter_map(|file| Some((file.path.as_str(), file.node.as_deref()?)))
                .collect(),
        )
    }
    pub(crate) fn of(&self, file: &str) -> Option<&'a str> {
        self.0.get(file).copied()
    }
    pub(crate) fn within(&self, file: &str, node: Option<&str>) -> bool {
        node.is_none_or(|node| self.of(file).is_some_and(|owner| is_within(owner, node)))
    }
}
