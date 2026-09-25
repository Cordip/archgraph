use crate::{
    baseline::{self, BaselineEntry},
    compiler, config, context,
    model::{ArchitectureIr, Violation, EVIDENCE_LIMIT},
    paths::locate_repository,
    projection,
    provider::{gitnexus::GitNexusCliProvider, ReindexMode},
    render,
    rules::violation_touches,
    server, suggest, vcs,
};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::{
    net::IpAddr,
    path::{Path, PathBuf},
};

/// `print!`/`println!` panic when stdout is closed early (`archgraph check |
/// head`). Like Unix tools killed by SIGPIPE, exit quietly with 128 + 13.
fn write_stdout(arguments: std::fmt::Arguments) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    if let Err(error) = stdout.write_fmt(arguments) {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(141);
        }
        panic!("failed printing to stdout: {error}");
    }
}
macro_rules! out {
    ($($arg:tt)*) => { write_stdout(format_args!($($arg)*)) };
}
macro_rules! outln {
    () => { write_stdout(format_args!("\n")) };
    ($($arg:tt)*) => { write_stdout(format_args!("{}\n", format_args!($($arg)*))) };
}

#[derive(Debug, Parser)]
#[command(
    name = "archgraph",
    version,
    about = "Compile and check recursive architecture over GitNexus",
    after_help = "One repository per configuration. Observed dependencies are the GitNexus relation kinds in provider.edge_types (default IMPORTS). Interfaces/manual edges are descriptive; UI is read-only. Symbol/AST exploration belongs to GitNexus, an external executable with its own license. No observed edge is not proof of no runtime dependency."
)]
pub struct Cli {
    /// Repository root; otherwise walk upward to a .git directory or worktree file.
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
    /// Architecture YAML, relative to repository root unless absolute.
    #[arg(long, global = true, default_value = "architecture.yaml")]
    pub config: PathBuf,
    /// Query the provider even if its index is unchanged since the last run.
    /// Results are otherwise reused from `.archgraph/cache/`.
    #[arg(long, global = true)]
    pub no_cache: bool,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Create architecture.yaml and optionally install the project-local agent skill.
    Init {
        #[arg(long)]
        install_skill: bool,
        /// Overwrite existing architecture YAML and existing ArchGraph skill files.
        #[arg(long)]
        force: bool,
        /// Draft nodes from the directory layout instead of a one-node starter,
        /// with coverage comments if a GitNexus index exists.
        #[arg(long)]
        suggest: bool,
    },
    /// Compile fresh, write deterministic IR, and report counts (violations do not fail compile).
    Compile {
        /// Refresh the provider index first: `--reindex` (incremental) or
        /// `--reindex=full` (after package.json/tsconfig changes).
        #[arg(long, value_enum, num_args = 0..=1, require_equals = true, default_missing_value = "incremental")]
        reindex: Option<ReindexMode>,
        #[arg(long)]
        json: bool,
    },
    /// Compile fresh and check all or a subtree. Exit: 0 clean, 1 failure, 2 violations.
    ///
    /// With a baseline file (see `baseline`), only observations missing from
    /// it count as violations.
    Check {
        node: Option<String>,
        /// Refresh the provider index first: `--reindex` (incremental) or
        /// `--reindex=full` (after package.json/tsconfig changes).
        #[arg(long, value_enum, num_args = 0..=1, require_equals = true, default_missing_value = "incremental")]
        reindex: Option<ReindexMode>,
        #[arg(long)]
        json: bool,
        /// Baseline file; default `<config stem>.baseline.json` next to the config.
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Report every violation, ignoring an existing baseline.
        #[arg(long, conflicts_with = "baseline")]
        no_baseline: bool,
    },
    /// Accept all current violations: write them to the baseline file so that
    /// `check` fails only on new ones. Commit the file; regenerate deliberately.
    Baseline {
        /// Refresh the provider index first: `--reindex` (incremental) or
        /// `--reindex=full` (after package.json/tsconfig changes).
        #[arg(long, value_enum, num_args = 0..=1, require_equals = true, default_missing_value = "incremental")]
        reindex: Option<ReindexMode>,
        #[arg(long)]
        baseline: Option<PathBuf>,
    },
    /// Compile fresh and render one semantic focus level.
    Show {
        node: Option<String>,
        #[arg(long, value_enum, default_value = "text")]
        format: ShowFormat,
    },
    /// Compile fresh and emit agent-oriented architecture context.
    Context {
        node: String,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = EVIDENCE_LIMIT)]
        evidence_limit: usize,
    },
    /// Compile once and serve a read-only focus UI. Restart to reload after edits.
    Serve {
        /// Refresh the provider index first: `--reindex` (incremental) or
        /// `--reindex=full` (after package.json/tsconfig changes).
        #[arg(long, value_enum, num_args = 0..=1, require_equals = true, default_missing_value = "incremental")]
        reindex: Option<ReindexMode>,
        /// Binding beyond loopback is explicit and exposes architecture metadata.
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        #[arg(long, default_value_t = 7331)]
        port: u16,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ShowFormat {
    Text,
    Json,
    Mermaid,
}

pub fn check_exit_code(violation_count: usize) -> u8 {
    if violation_count == 0 {
        0
    } else {
        2
    }
}

pub fn matching_violations<'a>(
    ir: &'a ArchitectureIr,
    node: Option<&str>,
) -> Result<Vec<&'a Violation>> {
    if let Some(id) = node {
        if !ir.nodes.contains_key(id) {
            bail!("unknown architecture node `{id}`; inspect `archgraph show` or use UI search");
        }
    }
    Ok(ir
        .violations
        .iter()
        .filter(|violation| node.is_none_or(|id| violation_touches(violation, id)))
        .collect())
}

pub async fn run(cli: Cli) -> Result<u8> {
    let cwd = std::env::current_dir().context("cannot read current directory")?;
    let root = locate_repository(cli.root.as_deref(), &cwd)?;
    let config_path = if cli.config.is_absolute() {
        cli.config.clone()
    } else {
        root.join(&cli.config)
    };
    if let Commands::Init {
        install_skill,
        force,
        suggest,
    } = &cli.command
    {
        let content = if *suggest && (*force || !config_path.exists()) {
            suggested_config(&root, cli.no_cache).await?
        } else {
            STARTER.into()
        };
        initialize(&root, &config_path, &content, *install_skill, *force)?;
        return Ok(0);
    }
    let validated = config::load(&config_path)?;
    let requested_node = match &cli.command {
        Commands::Check { node, .. } | Commands::Show { node, .. } => node.as_deref(),
        Commands::Context { node, .. } => Some(node.as_str()),
        _ => None,
    };
    if let Some(id) = requested_node {
        if !validated.config.nodes.contains_key(id) {
            bail!("unknown architecture node `{id}`; inspect architecture.yaml or use UI search");
        }
    }
    let provider = GitNexusCliProvider::new(&root, &validated.config.provider)?;
    let reindex = match &cli.command {
        Commands::Compile { reindex, .. }
        | Commands::Check { reindex, .. }
        | Commands::Baseline { reindex, .. }
        | Commands::Serve { reindex, .. } => *reindex,
        _ => None,
    };
    let options = compiler::CompileOptions {
        reindex,
        cache: (!cli.no_cache).then(|| root.join(".archgraph/cache/provider.json")),
    };
    let compiler::Compiled { ir, cached } =
        compiler::compile_with(&root, &validated, &provider, &options).await?;
    let ir_path = compiler::persist(&root, &ir)?;
    for warning in &ir.diagnostics.warnings {
        eprintln!("warning: {warning}");
    }
    match cli.command {
        Commands::Compile { json, .. } => {
            if json {
                outln!(
                    "{}",
                    render::json::render(&serde_json::json!({
                        "stats": ir.stats, "diagnostics": ir.diagnostics, "provider_cached": cached,
                        "ir_path": ir_path.to_string_lossy().replace('\\', "/"), "evidence_notice": ir.evidence_notice
                    }))?
                );
            } else {
                outln!(
                    "Architecture compiled\n  architecture nodes:  {}\n  mapped files:        {}\n  unassigned files:    {}\n  ambiguous files:     {}\n  provider rows:       {}{}\n  filtered out:        {}\n  observed edges:      {}\n  resolved edges:      {}\n  architecture edges:  {}\n  violations:          {}\n\nIR: {}",
                    ir.stats.architecture_node_count,
                    ir.stats.mapped_file_count,
                    ir.stats.unassigned_file_count,
                    ir.stats.ambiguous_file_count,
                    ir.stats.provider_row_count,
                    if cached { " (cached: index unchanged)" } else { "" },
                    ir.stats.filtered_edge_count,
                    ir.stats.observed_edge_count,
                    ir.stats.resolved_edge_count,
                    ir.stats.aggregated_architecture_edge_count,
                    ir.stats.violation_count,
                    ir_path.display()
                );
                outln!("\n{}", ir.evidence_notice);
            }
            Ok(0)
        }
        Commands::Check {
            node,
            json,
            baseline: baseline_arg,
            no_baseline,
            ..
        } => {
            let violations = matching_violations(&ir, node.as_deref())?;
            let baseline_path = baseline_file(&root, &config_path, baseline_arg);
            let loaded = if no_baseline {
                None
            } else {
                baseline::load(&baseline_path)?
            };
            let no_renames = std::collections::BTreeMap::new();
            let mut comparison = loaded
                .as_ref()
                .map(|loaded| baseline::compare(&ir, &violations, loaded, &no_renames));
            // Files moved since the baseline would otherwise turn accepted
            // observations into new ones.
            let mut rename_error = None;
            if let (Some(first), Some(loaded)) = (&comparison, &loaded) {
                if let (true, Some(commit)) = (first.may_involve_renames(), &loaded.commit) {
                    match vcs::renames_since(&root, commit) {
                        Ok(renames) if !renames.is_empty() => {
                            comparison =
                                Some(baseline::compare(&ir, &violations, loaded, &renames));
                        }
                        Ok(_) => {}
                        Err(error) => rename_error = Some(format!("{error:#}")),
                    }
                }
            }
            if let Some(error) = &rename_error {
                eprintln!("warning: renamed files cannot be matched to the baseline: {error}");
            }
            let failing: Vec<&Violation> = match &comparison {
                Some(comparison) => comparison.new.iter().map(|new| new.violation).collect(),
                None => violations.clone(),
            };
            let coverage_failures: Vec<&crate::model::CoverageIssue> =
                if validated.config.policies.low_coverage == config::FilePolicy::Error {
                    ir.diagnostics
                        .low_coverage
                        .iter()
                        .filter(|issue| {
                            node.as_deref()
                                .is_none_or(|id| config::overlaps(&issue.node, id))
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            // Unverified is not clean: coverage failures take precedence over
            // both a clean result and violations (exit 1, like other failures).
            let exit = if coverage_failures.is_empty() {
                check_exit_code(failing.len())
            } else {
                1
            };
            let new_entries = |violation: &Violation| -> Option<&[BaselineEntry]> {
                comparison.as_ref().and_then(|comparison| {
                    comparison
                        .new
                        .iter()
                        .find(|new| std::ptr::eq(new.violation, violation))
                        .map(|new| new.new_entries.as_slice())
                })
            };
            if json {
                let reported = failing
                    .iter()
                    .map(|&violation| {
                        let mut value = serde_json::to_value(violation)?;
                        if let Some(entries) = new_entries(violation) {
                            value["new_observations"] = serde_json::to_value(entries)?;
                        }
                        Ok(value)
                    })
                    .collect::<Result<Vec<_>>>()?;
                let baseline_json = comparison.as_ref().map(|comparison| serde_json::json!({
                    "path": baseline_path.to_string_lossy().replace('\\', "/"),
                    "accepted_observations": comparison.accepted_entry_count,
                    "fixed_observations": comparison.fixed_entry_count,
                    "renamed_observations": comparison.renamed_entry_count,
                    "accepted_violations": comparison.accepted.iter().map(|v| &v.rule_id).collect::<Vec<_>>(),
                }));
                outln!(
                    "{}",
                    render::json::render(
                        &serde_json::json!({"node": node, "violation_count": failing.len(),
                    "violations": reported, "baseline": baseline_json, "coverage_failures": coverage_failures,
                    "diagnostics": ir.diagnostics,
                    "evidence_notice": ir.evidence_notice,
                    "ir_path": ir_path.to_string_lossy().replace('\\', "/")})
                    )?
                );
            } else {
                if let Some(comparison) = &comparison {
                    outln!(
                        "Baseline {}: {} accepted observation(s), {} fixed since; {} violation(s) fully accepted.",
                        baseline_path.display(),
                        comparison.accepted_entry_count,
                        comparison.fixed_entry_count,
                        comparison.accepted.len()
                    );
                    if comparison.renamed_entry_count > 0 {
                        outln!(
                            "{} accepted observation(s) follow files renamed since the baseline; run `archgraph baseline` to record the new paths.",
                            comparison.renamed_entry_count
                        );
                    }
                    outln!(
                        "{} architecture violation(s) with observations not in the baseline",
                        failing.len()
                    );
                } else {
                    outln!("{} matching architecture violation(s)", failing.len());
                }
                for violation in failing {
                    match new_entries(violation) {
                        Some(entries) => print_new_observations(violation, entries),
                        None => print_violation(violation),
                    }
                }
                if !coverage_failures.is_empty() {
                    outln!(
                        "\nNot verified: coverage below policies.min_observed_ratio ({}):",
                        validated.config.policies.min_observed_ratio
                    );
                    for issue in &coverage_failures {
                        outln!("  {issue}");
                    }
                }
                outln!("\n{}", ir.evidence_notice);
            }
            if !coverage_failures.is_empty() {
                eprintln!(
                    "error: {} rule node(s) are too sparsely observed to verify (policies.low_coverage: error)",
                    coverage_failures.len()
                );
            }
            Ok(exit)
        }
        Commands::Baseline {
            baseline: baseline_arg,
            ..
        } => {
            let path = baseline_file(&root, &config_path, baseline_arg);
            let entries = baseline::current_entries(&ir);
            baseline::write(&path, &entries, vcs::head_commit(&root))?;
            outln!(
                "Accepted {} observation(s) from {} violation(s) in {}",
                entries.len(),
                ir.violations.len(),
                path.display()
            );
            Ok(0)
        }
        Commands::Show { node, format } => {
            let focus = node.as_deref().unwrap_or(&ir.project.root);
            let view = projection::project(&ir, focus, EVIDENCE_LIMIT)?;
            match format {
                ShowFormat::Text => out!("{}", render::text::render(&view)),
                ShowFormat::Json => outln!("{}", render::json::render(&view)?),
                ShowFormat::Mermaid => out!("{}", render::mermaid::render(&view)),
            }
            Ok(0)
        }
        Commands::Context {
            node,
            json,
            evidence_limit,
        } => {
            let result = context::build(&ir, &node, evidence_limit)?;
            if json {
                outln!("{}", render::json::render(&result)?);
            } else {
                out!("{}", context::markdown(&result));
            }
            Ok(0)
        }
        Commands::Serve { host, port, .. } => {
            if !host.is_loopback() {
                eprintln!("warning: binding to {host} exposes read-only architecture metadata to reachable clients; there is no authentication");
            }
            server::serve(ir, host, port).await?;
            Ok(0)
        }
        Commands::Init { .. } => Ok(0), // Handled before loading config/provider.
    }
}

fn baseline_file(root: &Path, config_path: &Path, explicit: Option<PathBuf>) -> PathBuf {
    match explicit {
        Some(path) if path.is_absolute() => path,
        Some(path) => root.join(path),
        None => baseline::default_path(config_path),
    }
}

fn evidence_line(evidence: &crate::model::EdgeEvidence) -> String {
    let detail = [
        evidence.reason.clone(),
        evidence
            .confidence
            .map(|value| format!("confidence {value}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!("  ({})", detail.join(", "))
    };
    format!("{} -> {}{detail}", evidence.from_file, evidence.to_file)
}

/// The one-line `message` suits JSON and agents. A cycle over several nodes
/// reads better as members, the cheapest cut and the layers it leaves.
fn print_violation_header(violation: &Violation) {
    if violation.suggested_cuts.is_empty() {
        outln!("\n[{}] {}", violation.rule_id, violation.message);
        return;
    }
    outln!(
        "\n[{}] cycle among immediate children of `{}`",
        violation.rule_id,
        violation.from.as_deref().unwrap_or_default()
    );
    outln!("  Members: {}", violation.nodes.join(", "));
    let cut: usize = violation.suggested_cuts.iter().map(|cut| cut.count).sum();
    outln!(
        "  Cheapest cut: {cut} of {} participating dependencies, most significant first:",
        violation.count
    );
    for cut in &violation.suggested_cuts {
        outln!("    {} -> {} × {}", cut.from, cut.to, cut.count);
    }
    outln!("  Layers after the cut, upper to lower:");
    outln!("    {}", violation.layer_order.join(" > "));
}

fn print_violation(violation: &Violation) {
    print_violation_header(violation);
    let is_cut = |from: &str, to: &str| {
        violation
            .suggested_cuts
            .iter()
            .any(|cut| cut.from == from && cut.to == to)
    };
    if !violation.suggested_cuts.is_empty() {
        outln!("  Evidence for the cut; other cycle edges are summarized:");
    }
    for edge in &violation.architecture_edges {
        let cut = is_cut(&edge.from, &edge.to);
        outln!(
            "  {} -> {} [{}] × {}{}",
            edge.from,
            edge.to,
            edge.kind,
            edge.count,
            if cut { " [CUT]" } else { "" }
        );
        if !violation.suggested_cuts.is_empty() && !cut {
            continue;
        }
        for evidence in &edge.evidence {
            outln!("    {}", evidence_line(evidence));
        }
        if edge.evidence.len() < edge.count {
            outln!(
                "    (showing {} of {} observations)",
                edge.evidence.len(),
                edge.count
            );
        }
    }
}

/// With a baseline, the observations to fix are exactly the new ones.
fn print_new_observations(violation: &Violation, entries: &[BaselineEntry]) {
    const SHOWN: usize = 50;
    print_violation_header(violation);
    outln!("  New since baseline ({}):", entries.len());
    for entry in entries.iter().take(SHOWN) {
        outln!(
            "    {} -> {} [{}]",
            entry.from_file,
            entry.to_file,
            entry.kind
        );
    }
    if entries.len() > SHOWN {
        outln!("    ... and {} more (use --json)", entries.len() - SHOWN);
    }
}

const STARTER: &str = r#"version: 1
project:
  name: my-project
  root: app
  source_roots: ["."]
  exclude:
    - "**/target/**"
    - "**/node_modules/**"
    - "**/.venv/**"
    - "**/__pycache__/**"
    - "**/dist/**"
provider:
  kind: gitnexus
  command: gitnexus
  repo: null
  page_size: 5000
  edge_types: [IMPORTS]
  exclude_reasons: [markdown-link]
policies:
  unassigned_files: warn
  ambiguous_mapping: error
nodes:
  app:
    title: Application
    description: Replace this description and add logical child nodes for your application.
    maps: ["**"]
edges: []
rules: []
"#;
const SKILL: &str = include_str!("../skills/archgraph/SKILL.md");

/// A draft from the directory layout, compiled once against the index (if
/// any) to annotate each node with its coverage.
async fn suggested_config(root: &Path, no_cache: bool) -> Result<String> {
    let draft = suggest::draft(root)?;
    let validated = config::parse(&draft.render(None, None))
        .context("internal error: the suggested architecture is invalid")?;
    let provider = GitNexusCliProvider::new(root, &validated.config.provider)?;
    let options = compiler::CompileOptions {
        reindex: None,
        cache: (!no_cache).then(|| root.join(".archgraph/cache/provider.json")),
    };
    let yaml = match compiler::compile_with(root, &validated, &provider, &options).await {
        Ok(compiled) => {
            let ir = &compiled.ir;
            let width = draft
                .nodes
                .iter()
                .map(|node| node.id.len())
                .max()
                .unwrap_or(0);
            outln!("Suggested nodes (files with an observed dependency / mapped files):");
            for node in &draft.nodes {
                if let Some(compiled) = ir.nodes.get(&node.id) {
                    let total = compiled.descendant_file_count;
                    outln!(
                        "  {:width$}  {:>6} / {:<6} {:>3}%",
                        node.id,
                        compiled.observed_file_count,
                        total,
                        (compiled.observed_file_count * 100)
                            .checked_div(total)
                            .unwrap_or(0)
                    );
                }
            }
            draft.render(Some(ir), None)
        }
        Err(error) => {
            eprintln!(
                "note: no coverage comments; the GitNexus index could not be read: {error:#}"
            );
            draft.render(
                None,
                Some("no readable GitNexus index. Run `gitnexus analyze --index-only`, then `archgraph init --suggest --force`."),
            )
        }
    };
    config::parse(&yaml).context("internal error: the suggested architecture is invalid")?;
    Ok(yaml)
}

pub fn initialize(
    root: &Path,
    config_path: &Path,
    content: &str,
    install_skill: bool,
    force: bool,
) -> Result<()> {
    if config_path.exists() && !force {
        outln!(
            "Keeping existing {} (use --force to overwrite, or --config for another file)",
            config_path.display()
        );
    } else {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(config_path, content)
            .with_context(|| format!("cannot write {}", config_path.display()))?;
        outln!("Created {}", config_path.display());
    }
    if install_skill {
        let mut supported: Vec<PathBuf> = [".claude", ".agents"]
            .into_iter()
            .map(|name| root.join(name))
            .filter(|path| path.is_dir())
            .collect();
        // No existing agent directory: use the project-local, vendor-neutral default.
        if supported.is_empty() {
            supported.push(root.join(".agents"));
        }
        for agent in supported {
            let directory = agent.join("skills/archgraph");
            // Refuse project-local paths that actually point outside the project.
            for ancestor in directory
                .ancestors()
                .take_while(|path| path.starts_with(root))
            {
                if ancestor.exists() && !ancestor.canonicalize()?.starts_with(root) {
                    bail!("skill directory {} points outside the repository; choose a project-local directory", ancestor.display());
                }
            }
            std::fs::create_dir_all(&directory)?;
            let destination = directory.join("SKILL.md");
            if destination.exists() && !force {
                outln!(
                    "Keeping existing {} (use --force to overwrite)",
                    destination.display()
                );
                continue;
            }
            if destination
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                bail!(
                    "refusing to overwrite symlink {}; replace it with a project-local skill file",
                    destination.display()
                );
            }
            std::fs::write(&destination, SKILL)
                .with_context(|| format!("cannot install skill {}", destination.display()))?;
            outln!("Installed {}", destination.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_exit_semantics() {
        assert_eq!(check_exit_code(0), 0);
        assert_eq!(check_exit_code(1), 2);
    }
    #[test]
    fn all_commands_parse_and_global_flags_work_after_subcommand() {
        for args in [
            vec!["archgraph", "init", "--install-skill"],
            vec!["archgraph", "compile", "--reindex"],
            vec!["archgraph", "check", "app.a", "--json"],
            vec!["archgraph", "show", "--format", "mermaid"],
            vec!["archgraph", "context", "app", "--evidence-limit", "50"],
            vec!["archgraph", "serve", "--root", "/tmp"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
    }
    #[test]
    fn skill_installation_preserves_existing_files_without_force() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".claude/skills/archgraph")).unwrap();
        std::fs::create_dir_all(root.join(".agents")).unwrap();
        let skill = root.join(".claude/skills/archgraph/SKILL.md");
        std::fs::write(&skill, "keep me").unwrap();
        let yaml = root.join("architecture.yaml");
        initialize(&root, &yaml, STARTER, true, false).unwrap();
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), "keep me");
        assert!(root.join(".agents/skills/archgraph/SKILL.md").exists());
        initialize(&root, &yaml, STARTER, true, true).unwrap();
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), SKILL);
        assert!(config::load(&yaml).is_ok());
    }
}
