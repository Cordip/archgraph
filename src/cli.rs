use crate::{
    compiler, config, context,
    model::{ArchitectureIr, Violation, EVIDENCE_LIMIT},
    paths::locate_repository,
    projection,
    provider::gitnexus::GitNexusCliProvider,
    render,
    rules::violation_touches,
    server,
};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::{
    net::IpAddr,
    path::{Path, PathBuf},
};

#[derive(Debug, Parser)]
#[command(
    name = "archgraph",
    version,
    about = "Compile and check recursive architecture over GitNexus",
    after_help = "MVP: one repository; automatically observed IMPORTS only. Interfaces/manual edges are descriptive; UI is read-only. Symbol/AST exploration belongs to GitNexus, an external executable with its own license. No observed edge is not proof of no runtime dependency."
)]
pub struct Cli {
    /// Repository root; otherwise walk upward to a .git directory or worktree file.
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
    /// Architecture YAML, relative to repository root unless absolute.
    #[arg(long, global = true, default_value = "architecture.yaml")]
    pub config: PathBuf,
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
    },
    /// Compile fresh, write deterministic IR, and report counts (violations do not fail compile).
    Compile {
        /// Run the configured provider's incremental analyze --index-only first.
        #[arg(long)]
        reindex: bool,
        #[arg(long)]
        json: bool,
    },
    /// Compile fresh and check all or a subtree. Exit: 0 clean, 1 failure, 2 violations.
    Check {
        node: Option<String>,
        #[arg(long)]
        reindex: bool,
        #[arg(long)]
        json: bool,
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
        #[arg(long)]
        reindex: bool,
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
    } = &cli.command
    {
        initialize(&root, &config_path, *install_skill, *force)?;
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
        | Commands::Serve { reindex, .. } => *reindex,
        _ => false,
    };
    let ir = compiler::compile(&root, &validated, &provider, reindex).await?;
    let ir_path = compiler::persist(&root, &ir)?;
    for warning in &ir.diagnostics.warnings {
        eprintln!("warning: {warning}");
    }
    match cli.command {
        Commands::Compile { json, .. } => {
            if json {
                println!(
                    "{}",
                    render::json::render(&serde_json::json!({
                        "stats": ir.stats, "diagnostics": ir.diagnostics,
                        "ir_path": ir_path.to_string_lossy().replace('\\', "/"), "evidence_notice": ir.evidence_notice
                    }))?
                );
            } else {
                println!(
                    "Architecture compiled\n  architecture nodes:  {}\n  mapped files:        {}\n  unassigned files:    {}\n  ambiguous files:     {}\n  provider rows:       {}\n  filtered out:        {}\n  observed edges:      {}\n  resolved edges:      {}\n  architecture edges:  {}\n  violations:          {}\n\nIR: {}",
                    ir.stats.architecture_node_count,
                    ir.stats.mapped_file_count,
                    ir.stats.unassigned_file_count,
                    ir.stats.ambiguous_file_count,
                    ir.stats.provider_row_count,
                    ir.stats.filtered_edge_count,
                    ir.stats.observed_edge_count,
                    ir.stats.resolved_edge_count,
                    ir.stats.aggregated_architecture_edge_count,
                    ir.stats.violation_count,
                    ir_path.display()
                );
                println!("\n{}", ir.evidence_notice);
            }
            Ok(0)
        }
        Commands::Check { node, json, .. } => {
            let violations = matching_violations(&ir, node.as_deref())?;
            let exit = check_exit_code(violations.len());
            if json {
                println!(
                    "{}",
                    render::json::render(
                        &serde_json::json!({"node": node, "violation_count": violations.len(),
                    "violations": violations, "diagnostics": ir.diagnostics, "evidence_notice": ir.evidence_notice,
                    "ir_path": ir_path.to_string_lossy().replace('\\', "/")})
                    )?
                );
            } else {
                println!("{} matching architecture violation(s)", violations.len());
                for violation in violations {
                    println!("\n[{}] {}", violation.rule_id, violation.message);
                    for edge in &violation.architecture_edges {
                        println!(
                            "  {} -> {} [{}] × {}",
                            edge.from, edge.to, edge.kind, edge.count
                        );
                        for evidence in &edge.evidence {
                            println!("    {} -> {}", evidence.from_file, evidence.to_file);
                        }
                        if edge.evidence.len() < edge.count {
                            println!(
                                "    (showing {} of {} observations)",
                                edge.evidence.len(),
                                edge.count
                            );
                        }
                    }
                }
                println!("\n{}", ir.evidence_notice);
            }
            Ok(exit)
        }
        Commands::Show { node, format } => {
            let focus = node.as_deref().unwrap_or(&ir.project.root);
            let view = projection::project(&ir, focus, EVIDENCE_LIMIT)?;
            match format {
                ShowFormat::Text => print!("{}", render::text::render(&view)),
                ShowFormat::Json => println!("{}", render::json::render(&view)?),
                ShowFormat::Mermaid => print!("{}", render::mermaid::render(&view)),
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
                println!("{}", render::json::render(&result)?);
            } else {
                print!("{}", context::markdown(&result));
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

pub fn initialize(root: &Path, config_path: &Path, install_skill: bool, force: bool) -> Result<()> {
    if config_path.exists() && !force {
        println!("Keeping existing {}", config_path.display());
    } else {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(config_path, STARTER)
            .with_context(|| format!("cannot write {}", config_path.display()))?;
        println!("Created {}", config_path.display());
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
                println!(
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
            println!("Installed {}", destination.display());
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
        initialize(&root, &yaml, true, false).unwrap();
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), "keep me");
        assert!(root.join(".agents/skills/archgraph/SKILL.md").exists());
        initialize(&root, &yaml, true, true).unwrap();
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), SKILL);
        assert!(config::load(&yaml).is_ok());
    }
}
