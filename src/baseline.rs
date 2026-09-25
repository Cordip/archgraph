//! Accepted violations for legacy code. A baseline records every observation
//! behind the current violations; `check` then fails only on observations
//! that are not in it, so existing debt does not block CI while new debt does.
use crate::{
    model::{ArchitectureIr, EdgeEvidence, Violation},
    rules::violation_observations,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub const SCHEMA_VERSION: u32 = 1;

/// One accepted observation: a file-level dependency that violates a rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BaselineEntry {
    pub rule_id: String,
    pub from_file: String,
    pub to_file: String,
    pub kind: String,
}

impl From<(&str, &EdgeEvidence)> for BaselineEntry {
    fn from((rule_id, evidence): (&str, &EdgeEvidence)) -> Self {
        Self {
            rule_id: rule_id.into(),
            from_file: evidence.from_file.clone(),
            to_file: evidence.to_file.clone(),
            kind: evidence.kind.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub schema_version: u32,
    pub note: String,
    pub entries: Vec<BaselineEntry>,
}

/// `architecture.yaml` -> `architecture.baseline.json`, next to the config.
pub fn default_path(config_path: &Path) -> PathBuf {
    let stem = config_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "architecture".into());
    config_path.with_file_name(format!("{stem}.baseline.json"))
}

/// All observations behind one violation, as baseline entries.
pub fn violation_entries(ir: &ArchitectureIr, violation: &Violation) -> BTreeSet<BaselineEntry> {
    let Some(rule) = ir.rules.iter().find(|rule| rule.id() == violation.rule_id) else {
        return BTreeSet::new();
    };
    violation_observations(rule, violation, &ir.resolved_edges)
        .into_iter()
        .map(|edge| BaselineEntry::from((violation.rule_id.as_str(), &edge.evidence)))
        .collect()
}

pub fn current_entries(ir: &ArchitectureIr) -> BTreeSet<BaselineEntry> {
    ir.violations
        .iter()
        .flat_map(|violation| violation_entries(ir, violation))
        .collect()
}

pub fn load(path: &Path) -> Result<Option<Baseline>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };
    let baseline: Baseline = serde_json::from_str(&text).with_context(|| {
        format!(
            "{} is not a valid ArchGraph baseline; regenerate it with `archgraph baseline`",
            path.display()
        )
    })?;
    if baseline.schema_version != SCHEMA_VERSION {
        bail!(
            "{}: unsupported baseline schema {}; expected {SCHEMA_VERSION}",
            path.display(),
            baseline.schema_version
        );
    }
    Ok(Some(baseline))
}

pub fn write(path: &Path, entries: &BTreeSet<BaselineEntry>) -> Result<()> {
    let baseline = Baseline {
        schema_version: SCHEMA_VERSION,
        note: "Accepted architecture violations. `archgraph check` fails only on observations not listed here. Regenerate deliberately with `archgraph baseline`.".into(),
        entries: entries.iter().cloned().collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&baseline)?;
    bytes.push(b'\n');
    std::fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))
}

/// A violation that has observations the baseline does not accept.
#[derive(Debug)]
pub struct NewViolation<'a> {
    pub violation: &'a Violation,
    pub new_entries: Vec<BaselineEntry>,
}

#[derive(Debug)]
pub struct Comparison<'a> {
    pub new: Vec<NewViolation<'a>>,
    pub accepted: Vec<&'a Violation>,
    pub accepted_entry_count: usize,
    /// Baseline entries no longer observed anywhere: debt that was paid off.
    pub fixed_entry_count: usize,
}

pub fn compare<'a>(
    ir: &ArchitectureIr,
    violations: &[&'a Violation],
    baseline: &Baseline,
) -> Comparison<'a> {
    let accepted: BTreeSet<&BaselineEntry> = baseline.entries.iter().collect();
    let current = current_entries(ir);
    let mut comparison = Comparison {
        new: Vec::new(),
        accepted: Vec::new(),
        accepted_entry_count: baseline.entries.len(),
        fixed_entry_count: baseline
            .entries
            .iter()
            .filter(|entry| !current.contains(entry))
            .count(),
    };
    for &violation in violations {
        let new_entries: Vec<_> = violation_entries(ir, violation)
            .into_iter()
            .filter(|entry| !accepted.contains(entry))
            .collect();
        if new_entries.is_empty() {
            comparison.accepted.push(violation);
        } else {
            comparison.new.push(NewViolation {
                violation,
                new_entries,
            });
        }
    }
    comparison
}
