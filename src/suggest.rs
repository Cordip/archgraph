//! `init --suggest`: a draft architecture from the directory layout. It cannot
//! know responsibilities, only where code is; the draft says so and asks the
//! user to rename, merge and describe nodes. With an index available, the CLI
//! compiles the draft once and annotates every node with its coverage, so weak
//! spots (unparsed languages, autoloaded code) are visible before any rule is
//! written.
use crate::{
    config::{self, valid_node_id},
    discovery,
    model::ArchitectureIr,
};
use anyhow::{Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::Path,
};

/// File types GitNexus can extract dependencies from, or plausibly will.
const CODE_EXTENSIONS: &[&str] = &[
    "c", "cc", "cjs", "clj", "cljs", "coffee", "cpp", "cs", "cts", "cxx", "dart", "erl", "ex",
    "exs", "go", "h", "hh", "hpp", "hxx", "java", "js", "jsx", "kt", "kts", "lua", "m", "mjs",
    "mm", "mts", "php", "py", "pyi", "rake", "rb", "rs", "scala", "svelte", "swift", "ts", "tsx",
    "vue",
];
const RUBY_EXTENSIONS: &[&str] = &["rake", "rb"];
/// Well-known file types without code dependencies. Only those present are
/// excluded; unknown types stay mapped rather than guessed about.
const NON_CODE_EXTENSIONS: &[&str] = &[
    "adoc",
    "asc",
    "avif",
    "bmp",
    "cfg",
    "conf",
    "crt",
    "csr",
    "csv",
    "ejs",
    "env",
    "eot",
    "erb",
    "eco",
    "gif",
    "gql",
    "graphql",
    "haml",
    "hbs",
    "htm",
    "html",
    "ico",
    "ini",
    "jpeg",
    "jpg",
    "json",
    "jst",
    "key",
    "less",
    "lock",
    "log",
    "map",
    "md",
    "mov",
    "mp3",
    "mp4",
    "mustache",
    "ogg",
    "otf",
    "pdf",
    "pem",
    "pgp",
    "png",
    "po",
    "pot",
    "properties",
    "proto",
    "rst",
    "rtf",
    "sass",
    "scss",
    "sh",
    "slim",
    "snap",
    "sql",
    "styl",
    "svg",
    "tiff",
    "toml",
    "tsv",
    "ttf",
    "txt",
    "wav",
    "webm",
    "webp",
    "woff",
    "woff2",
    "xml",
    "yaml",
    "yml",
];
/// Third-party, generated and tool directories; hidden directories as well.
const BASE_EXCLUDE: &[&str] = &[
    "**/target/**",
    "**/node_modules/**",
    "**/vendor/**",
    "**/.venv/**",
    "**/__pycache__/**",
    "**/dist/**",
    "**/.*/**",
];
/// A directory becomes a node only with this share of all code files...
const MIN_SHARE: usize = 40;
/// ...and at least this many.
const MIN_FILES: usize = 5;
const MAX_DEPTH: usize = 3;
/// Share of a directory's code that makes a single child a mere wrapper
/// (such as `src/`), which is descended into instead of becoming a node.
const WRAPPER_PERCENT: usize = 80;
const LOW_COVERAGE_PERCENT: usize = 50;

#[derive(Debug, Clone, PartialEq)]
pub struct SuggestedNode {
    pub id: String,
    /// Repository-relative directory; empty for the root node.
    pub path: String,
    pub code_files: usize,
}

#[derive(Debug)]
pub struct Draft {
    pub project: String,
    pub nodes: Vec<SuggestedNode>,
    /// `no_cycles` scopes: nodes with at least two children.
    pub cycle_scopes: Vec<String>,
    pub non_code_extensions: Vec<String>,
    pub ruby: bool,
    /// Stylesheets next to scripts: observe CSS (`provider.css`).
    pub css: bool,
    pub min_files: usize,
}

fn extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
}

fn segment(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        "dir".into()
    } else {
        cleaned.into()
    }
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

struct Tree {
    /// Code files at or below each directory ("" is the repository root).
    counts: BTreeMap<String, usize>,
    children: BTreeMap<String, BTreeSet<String>>,
}

impl Tree {
    fn new(code_files: &[&str]) -> Self {
        let mut tree = Self {
            counts: BTreeMap::new(),
            children: BTreeMap::new(),
        };
        for file in code_files {
            let mut directory = parent_dir(file);
            loop {
                *tree.counts.entry(directory.into()).or_default() += 1;
                if directory.is_empty() {
                    break;
                }
                let parent = parent_dir(directory);
                tree.children
                    .entry(parent.into())
                    .or_default()
                    .insert(directory.into());
                directory = parent;
            }
        }
        tree
    }
    fn count(&self, directory: &str) -> usize {
        self.counts.get(directory).copied().unwrap_or(0)
    }
    fn split(&self, directory: &str, id: &str, depth: usize, min_files: usize, draft: &mut Draft) {
        let mut directory = directory.to_owned();
        let candidates = loop {
            let candidates: Vec<&String> = self
                .children
                .get(&directory)
                .into_iter()
                .flatten()
                .filter(|child| self.count(child) >= min_files)
                .collect();
            match candidates.as_slice() {
                [only] if self.count(only) * 100 >= self.count(&directory) * WRAPPER_PERCENT => {
                    directory = (*only).clone();
                }
                _ => break candidates,
            }
        };
        if candidates.len() < 2 || depth >= MAX_DEPTH {
            return;
        }
        draft.cycle_scopes.push(id.into());
        let mut used = BTreeSet::new();
        for child in candidates {
            let name = child.rsplit('/').next().unwrap_or(child);
            let base = segment(name);
            let mut unique = base.clone();
            let mut suffix = 2;
            while !used.insert(unique.clone()) {
                unique = format!("{base}-{suffix}");
                suffix += 1;
            }
            let child_id = format!("{id}.{unique}");
            draft.nodes.push(SuggestedNode {
                id: child_id.clone(),
                path: child.clone(),
                code_files: self.count(child),
            });
            self.split(child, &child_id, depth + 1, min_files, draft);
        }
    }
}

/// Every file `compile` would see under the draft's base exclusions.
fn repository_files(root: &Path) -> Result<Vec<String>> {
    let exclude: Vec<String> = BASE_EXCLUDE
        .iter()
        .map(|glob| serde_json::to_string(glob).expect("string serializes"))
        .collect();
    let yaml = format!(
        "version: 1\nproject: {{name: draft, root: draft, source_roots: [\".\"], exclude: [{}]}}\nprovider: {{kind: gitnexus}}\nnodes: {{draft: {{}}}}\n",
        exclude.join(", ")
    );
    discovery::discover(root, &config::parse(&yaml)?)
}

pub fn draft(root: &Path) -> Result<Draft> {
    let root = root
        .canonicalize()
        .context("cannot resolve repository root")?;
    let files = repository_files(&root)?;
    let is_code = |file: &&String| {
        extension(file).is_some_and(|extension| CODE_EXTENSIONS.contains(&extension.as_str()))
    };
    let code: Vec<&str> = files.iter().filter(is_code).map(String::as_str).collect();
    let non_code_extensions: BTreeSet<String> = files
        .iter()
        .filter_map(|file| extension(file))
        .filter(|extension| NON_CODE_EXTENSIONS.contains(&extension.as_str()))
        .collect();
    let ruby_files = code
        .iter()
        .filter(|file| {
            extension(file).is_some_and(|extension| RUBY_EXTENSIONS.contains(&extension.as_str()))
        })
        .count();
    let name = root.file_name().map_or_else(
        || "project".into(),
        |name| name.to_string_lossy().into_owned(),
    );
    let project = segment(&name);
    let min_files = MIN_FILES.max(code.len() / MIN_SHARE);
    let mut draft = Draft {
        project: project.clone(),
        nodes: vec![SuggestedNode {
            id: project.clone(),
            path: String::new(),
            code_files: code.len(),
        }],
        cycle_scopes: Vec::new(),
        non_code_extensions: non_code_extensions.into_iter().collect(),
        ruby: ruby_files * 10 >= code.len().max(1),
        css: files.iter().any(|file| file.ends_with(".css"))
            && code.iter().any(|file| {
                extension(file).is_some_and(|ext| {
                    ["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"].contains(&ext.as_str())
                })
            }),
        min_files,
    };
    Tree::new(&code).split("", &project, 0, min_files, &mut draft);
    debug_assert!(draft.nodes.iter().all(|node| valid_node_id(&node.id)));
    Ok(draft)
}

fn quoted(text: &str) -> String {
    serde_json::to_string(text).expect("string serializes")
}

fn coverage_comment(ir: &ArchitectureIr, id: &str) -> Option<String> {
    let node = ir.nodes.get(id)?;
    let total = node.descendant_file_count;
    if total == 0 {
        return Some("no mapped files".into());
    }
    let percent = node.observed_file_count * 100 / total;
    let mut comment = format!(
        "{} of {total} files have an observed dependency ({percent}%)",
        node.observed_file_count
    );
    if node.unindexed_file_count > 0 {
        let _ = write!(comment, ", {} not indexed", node.unindexed_file_count);
    }
    if percent < LOW_COVERAGE_PERCENT {
        comment.push_str("; checks here are weak");
    }
    Some(comment)
}

impl Draft {
    fn edge_types(&self) -> String {
        let mut kinds = vec!["IMPORTS"];
        if self.ruby {
            kinds.extend(["CALLS", "EXTENDS", "IMPLEMENTS"]);
        }
        if self.css {
            kinds.push("USES_CLASS");
        }
        format!("[{}]", kinds.join(", "))
    }

    /// `coverage` is the draft compiled against the index, if one exists;
    /// `note` explains a missing one.
    pub fn render(&self, coverage: Option<&ArchitectureIr>, note: Option<&str>) -> String {
        let mut yaml = String::new();
        let mut line = |text: &str| {
            yaml.push_str(text);
            yaml.push('\n');
        };
        line("# Draft generated by `archgraph init --suggest` from the directory layout.");
        line(&format!(
            "# A directory became a node if it holds at least {} code files.",
            self.min_files
        ));
        line("# Directories are not responsibilities: rename nodes, merge and split them,");
        line("# describe each one, and replace the generic no_cycles rules with real ones.");
        match (coverage, note) {
            (Some(_), _) => {
                line("# Coverage comments come from the current GitNexus index. A node where");
                line("# few files have an observed dependency cannot meaningfully fail a rule.");
            }
            (None, Some(note)) => line(&format!("# No coverage comments: {note}")),
            (None, None) => {}
        }
        line("version: 1");
        line("project:");
        line(&format!("  name: {}", quoted(&self.project)));
        line(&format!("  root: {}", self.project));
        line("  source_roots: [\".\"]");
        line("  exclude:");
        for glob in BASE_EXCLUDE {
            line(&format!("    - {}", quoted(glob)));
        }
        if !self.non_code_extensions.is_empty() {
            line("    # Common non-code file types found here; nothing to check in them.");
            line(&format!(
                "    - {}",
                quoted(&format!("**/*.{{{}}}", self.non_code_extensions.join(",")))
            ));
        }
        line("provider:");
        line("  kind: gitnexus");
        line("  command: gitnexus");
        line("  repo: null");
        line("  page_size: 5000");
        if self.ruby {
            line("  # Ruby constants are autoloaded, so Ruby dependencies appear as CALLS,");
            line("  # EXTENDS and IMPLEMENTS rather than IMPORTS. min_confidence 0.6 drops");
            line("  # GitNexus's global-name-fallback guesses (0.5).");
        } else {
            line("  # Add CALLS, EXTENDS and IMPLEMENTS where imports do not show dependencies.");
        }
        line(&format!("  edge_types: {}", self.edge_types()));
        line("  exclude_reasons: [markdown-link]");
        if self.css {
            line("  # GitNexus parses no CSS; ArchGraph reads stylesheets and class names");
            line("  # itself (IMPORTS of stylesheets, USES_CLASS; see `archgraph styles`).");
            line("  css: true");
        }
        if self.ruby {
            line("  min_confidence: 0.6");
        }
        line("policies:");
        line("  unassigned_files: warn");
        line("  ambiguous_mapping: error");
        line("nodes:");
        for node in &self.nodes {
            if let Some(comment) = coverage.and_then(|ir| coverage_comment(ir, &node.id)) {
                line(&format!("  # {comment}"));
            }
            line(&format!("  {}:", node.id));
            if node.path.is_empty() {
                line(&format!("    title: {}", quoted(&self.project)));
                line("    maps: [\"**\"]");
            } else {
                let name = node.path.rsplit('/').next().unwrap_or(&node.path);
                line(&format!("    title: {}", quoted(name)));
                line(&format!(
                    "    description: {}",
                    quoted(&format!(
                        "Draft: {} code files under {}/. Describe its responsibility.",
                        node.code_files, node.path
                    ))
                ));
                line(&format!(
                    "    maps: [{}]",
                    quoted(&format!("{}/**", node.path))
                ));
            }
        }
        line("edges: []");
        if self.cycle_scopes.is_empty() {
            line("rules: []");
        } else {
            line("rules:");
            for scope in &self.cycle_scopes {
                line(&format!(
                    "  - {{id: {}, kind: no_cycles, within: {scope}, edge_types: {}}}",
                    quoted(&format!("no-cycles-in-{}", scope.replace('.', "-"))),
                    self.edge_types()
                ));
            }
        }
        yaml
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository(files: &[&str]) -> tempfile::TempDir {
        let temp = tempfile::Builder::new().prefix("shop").tempdir().unwrap();
        for file in files {
            let path = temp.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        temp
    }

    fn many(directory: &str, extension: &str, count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("{directory}/f{index}.{extension}"))
            .collect()
    }

    #[test]
    fn nodes_follow_code_directories_through_wrappers() {
        let mut files: Vec<String> = Vec::new();
        files.extend(many("src/api", "ts", 8));
        files.extend(many("src/domain/orders", "ts", 6));
        files.extend(many("src/domain/billing", "ts", 6));
        files.extend(many("src/tiny", "ts", 2));
        files.extend(many("docs", "md", 30));
        files.push("spec/fixtures/token.created_at".into());
        files.push("package.json".into());
        files.push("src/styles.css".into());
        files.extend(many("node_modules/lib", "js", 50));
        files.extend(many(".github/workflows", "js", 20));
        files.push("src/weird name.v2/x.ts".into());
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let temp = repository(&refs);
        let draft = draft(temp.path()).unwrap();
        let root = draft.project.clone();
        let ids: Vec<&str> = draft.nodes.iter().map(|node| node.id.as_str()).collect();
        // `src/` holds everything, so it is a wrapper, not a node; `tiny` is
        // too small; vendored and hidden directories are not counted.
        assert_eq!(
            ids,
            [
                root.clone(),
                format!("{root}.api"),
                format!("{root}.domain"),
                format!("{root}.domain.billing"),
                format!("{root}.domain.orders"),
            ]
        );
        assert_eq!(draft.cycle_scopes, [root.clone(), format!("{root}.domain")]);
        assert_eq!(draft.non_code_extensions, ["json", "md"]);
        assert!(!draft.ruby);
        assert!(draft.css);

        let yaml = draft.render(None, Some("no index"));
        let validated = config::parse(&yaml).unwrap();
        assert_eq!(validated.config.rules.len(), 2);
        assert!(yaml.contains("maps: [\"src/domain/orders/**\"]"), "{yaml}");
        assert!(yaml.contains("\"**/*.{json,md}\""), "{yaml}");
        assert!(yaml.contains("# No coverage comments: no index"), "{yaml}");
        assert!(validated.config.provider.css);
        assert!(validated
            .config
            .provider
            .edge_types
            .contains(&"USES_CLASS".to_owned()));
    }

    #[test]
    fn ruby_code_observes_calls_and_odd_names_become_valid_ids() {
        let mut files: Vec<String> = Vec::new();
        files.extend(many("app/models", "rb", 10));
        files.extend(many("app/my.jobs", "rb", 10));
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let temp = repository(&refs);
        let draft = draft(temp.path()).unwrap();
        assert!(draft.ruby);
        assert!(draft.nodes.iter().any(|node| node.id.ends_with(".my-jobs")));
        let yaml = draft.render(None, None);
        let validated = config::parse(&yaml).unwrap();
        assert_eq!(validated.config.provider.edge_types.len(), 4);
        assert_eq!(validated.config.provider.min_confidence, Some(0.6));
    }

    #[test]
    fn an_empty_repository_still_yields_a_valid_draft() {
        let temp = repository(&["README"]);
        let draft = draft(temp.path()).unwrap();
        assert_eq!(draft.nodes.len(), 1);
        assert!(config::parse(&draft.render(None, None)).is_ok());
    }
}
