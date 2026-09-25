# ArchGraph MVP implementation report

## Delivery

This is a new, single-crate Rust implementation in `archgraph/`. The workspace
contained the supplied design document but no existing source repository, so
there was no prior application code to modify. The binary name is `archgraph`.
The original design is preserved in `DESIGN.md`, followed by explicit
implementation clarifications. All required MVP areas have source
implementations; there are no placeholder commands or HTTP handlers.

**Rust toolchain unavailable: implementation and tests were written but not
compiled/executed in this environment.** The Rust validation described below is
static review, not a successful build, test run, or formatting pass.

## Commands

| Command | Implemented behavior |
| --- | --- |
| `archgraph init` | Non-destructive starter YAML; `--install-skill`; explicit `--force` replacement. |
| `archgraph compile` | Fresh compilation, diagnostics, deterministic persisted IR, counts, `--json`, `--reindex`. Rule violations do not make this command fail. |
| `archgraph check [NODE]` | Fresh compilation, optional subtree filtering, text or `--json`; exit 0 for no matching violations, 1 for failure, 2 for matching violations; `--reindex`. CLI usage errors also exit 1, not 2. |
| `archgraph show [NODE]` | Shared semantic focus projection; `--format text`, `json`, or `mermaid`. Defaults to the configured project root. |
| `archgraph context NODE` | Agent-oriented Markdown or `--json`; configurable `--evidence-limit`; interfaces, counts, dependency direction, rules, violations, concrete evidence, and agent contract. |
| `archgraph serve` | Compile-on-start read-only snapshot, embedded HTML/CSS/JavaScript, `--reindex`, `--host`, `--port`; loopback by default. |

`--root` and `--config` are global options and work before or after a
subcommand. `show` and `context` also compile fresh rather than trusting a
possibly stale persisted IR. Operational errors propagate to the top-level
exit-1 handler. No command substitutes an empty provider result after a failure.

## Implementation map and acceptance coverage

| Area | Files | Implemented acceptance criteria |
| --- | --- | --- |
| Crate/entry point | `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/error.rs` | One Rust crate; explicit error boundary; no unsafe code; no database, LLM client, language parser, or frontend build. |
| Schema/hierarchy | `src/config.rs` | Version 1 schema, project/provider/policies, arbitrary dotted nodes, optional metadata/interfaces, manual typed edges, three rule variants, actionable reference/parent/glob validation. |
| Paths/discovery | `src/paths.rs`, `src/discovery.rs` | Repository location, source roots, exclusions, repository ignore semantics, UTF-8 forward-slash paths, Windows verbatim/UNC root handling, no Git internals traversal. |
| Membership | `src/mapping.rs` | Compiled glob sets, deepest match in an ancestor chain, unrelated-branch ambiguity, unassigned policy, deterministic diagnostics. |
| Provider abstraction | `src/provider/mod.rs` | Object-safe async `CodeGraphProvider` and injectable in-memory test provider. Reindexing also goes through the abstraction. |
| GitNexus boundary | `src/provider/gitnexus.rs`, `src/provider/markdown_table.rs` | Executable override, separate argv, repository cwd, optional repo argument, incremental reindex, version/probe, targeted paginated imports, strict JSON wrapper/table adapter, escaped pipes, finite optional confidence, fail-closed errors. |
| Compiler/IR | `src/compiler.rs`, `src/model.rs` | Membership and observed file graph resolution, anomaly diagnostics, parent file counts, observed/manual separation, aggregation, full counts/confidence ranges, bounded deterministic evidence, rules, timestamp-free JSON, `.archgraph/architecture.ir.json`. |
| Rules | `src/rules.rs` | `deny_dependency`, `allow_only`, immediate-child `no_cycles`, iterative Kosaraju, self-edge omission, SCC and per-architecture-edge concrete evidence, precise scoped violation ownership. |
| Shared projection | `src/projection.rs` | Immediate children or leaf files, lossless edge projection, collapsed self-edge omission, aggregated duplicates, cross-boundary entries, breadcrumbs, violation annotations. |
| Agent/rendering | `src/context.rs`, `src/render/*` | Markdown/JSON context, text/JSON/Mermaid focus output, deterministic concrete examples, descriptions/interfaces, incoming/outgoing dependencies, affected rules, post-edit verification contract. |
| CLI/init | `src/cli.rs` | All six commands, fresh compilation, required exit statuses, summaries, reindex options, skill installation and preservation. |
| HTTP/browser | `src/server.rs`, `src/web/*` | Read-only Axum 0.8 routes, loopback default, embedded assets, SVG directed graphs, breadcrumbs, click/double-click navigation, search, focus deep links, details/evidence panels, explicit violation markers, safely created DOM text. |
| Agent skill | `skills/archgraph/SKILL.md` | Context-first workflow, GitNexus symbol investigation, edit/reindex/check loop, no completion on exit 2, no architecture edits without explicit authorization. |
| Documentation | `README.md`, `DESIGN.md`, `architecture.example.yaml`, `LICENSE` | Quick start, full example, all commands, desired vs observed architecture, limitations, first-build instructions, provider licensing boundary. |

The safe HTTP routes are `/api/meta`, `/api/nodes`, `/api/focus/{node_id}`,
`/api/violations`, and `/api/search?q=...`. The server offers no arbitrary Cypher,
source-file content, or mutation endpoints. Responses carry a restrictive
Content Security Policy, `nosniff`, no-referrer, and no-store headers. Binding
beyond loopback requires explicit configuration and prints a warning; there is
no authentication.

## Tests written

There are **53 Rust test functions**: 35 unit tests and 18 integration tests.
They were not run here. Some functions check multiple configurations or CLI
invocations.

Unit coverage includes node ID validity, missing parents, segment-aware
ancestry, node/reference/version/glob validation, Windows and Unix paths,
repository/worktree location, file discovery and ignore handling, deepest-match
mapping, ambiguous/unassigned policies, aggregation, sorted/capped evidence,
all three rules, SCC behavior, provider wrapper compatibility, escaped-pipe
Markdown parsing, malformed rows, confidence parsing, CLI argument/exit
semantics, skill preservation, and Mermaid label escaping.

`tests/compiler_projection.rs` adds in-memory provider compilation, provider
failure propagation, IR serialization/persistence, repeated/shuffled-input
determinism, unmappable provider-edge diagnostics, semantic projection,
cross-boundary/manual edges, direct ownership, leaf graph preservation beyond
the 20-example aggregate cap, larger context evidence limits, actual-owner
scoped checks, and read-only Axum route tests.

`tests/cli.rs` adds Unix process-level checks for exits 0/1/2, invalid arguments,
configuration/provider failures, JSON output, Mermaid, pagination termination,
repository cwd, spaced executable/repository arguments, and incremental
reindexing without corrupting JSON stdout. These tests use the explicitly
test-only `tests/fixtures/fake_gitnexus.py` executable and require Python 3. Core
Rust tests do not require GitNexus or Python. Production ArchGraph is Rust and
embedded JavaScript, not Python.

The separate optional `tests/web_smoke.py` harness exercises the embedded
browser code in an isolated DOM with mocked fetch and history interfaces. It
is not part of `cargo test`, not a production dependency, and not evidence of a
working Rust server.

## Clarifications and bounded deviations

1. **Explicit external parent.** The supplied example omitted the parent of
   `external.stripe`, while the specification requires all dotted parents to
   exist. The distributed example declares `external` explicitly. External
   nodes cannot map files or contain internal ownership nodes.
2. **Files directly owned by non-leaf nodes.** Such mappings remain legal. The
   projection adds a synthetic direct-files entry rather than hiding these
   files. A manual edge naming the focus itself uses a synthetic boundary
   entry rather than inventing a file endpoint.
3. **Lossless resolved imports in the IR.** In addition to bounded aggregate
   evidence, the IR stores resolved file imports. Otherwise file-level zoom and
   larger agent evidence requests could silently lose observations. This does
   not copy the provider's whole symbol graph or any source contents.
4. **Ambiguity warning mode.** Ambiguous files remain explicitly ambiguous and
   unmapped; no arbitrary branch is selected.
5. **Scope of checks.** A violation records actual affected ownership nodes as
   well as SCC display nodes. A deep subtree is not flagged merely because an
   unaffected sibling shares an ancestor with a violation.
6. **Agent directory fallback.** Existing `.claude` and `.agents` project
   directories are supported. When neither exists, installation creates the
   vendor-neutral `.agents/skills/archgraph` path. Existing skill files and YAML
   are preserved without `--force`.
7. **Repository-local discovery.** `.git`, `.archgraph`, and `.gitnexus` are
   never crawled. Global Git ignore files are disabled for reproducibility;
   repository ignore semantics remain enabled. Symlink source roots and
   non-UTF-8 paths fail with actionable errors instead of escaping discovery.
8. **Unresolved lockfile.** No `Cargo.lock` or built binary was fabricated
   without Cargo. Dependency resolution and a generated lockfile belong to the
   first real build.

No required command, rule, provider adapter, context surface, or UI feature was
intentionally omitted. This statement describes source coverage, not proven
runtime correctness.

## Validation actually performed

| Validation | Status and scope |
| --- | --- |
| Full design review and deliberate Rust static review | Performed, including module paths, crate usage/features, ownership/error boundaries, pagination, rule scope, evidence, serialization, and Axum 0.8 APIs. This does not establish that Rust compiles. |
| Rust compilation, Rust tests, rustfmt, Clippy | **Not run: the Rust toolchain is unavailable.** |
| Real GitNexus probe/query/indexing | **Not run: no usable live GitNexus installation/index was available.** |
| `node --check src/web/app.js` | Passed JavaScript syntax checking. |
| Cargo manifest and YAML examples | Parsed with Python TOML/YAML libraries; example IDs, parents, external mapping restrictions, manual endpoints, and rule references checked. Rust/Serde execution remains untested. |
| Source structure and embedded assets | All out-of-line module declarations and embedded asset paths checked; required-functionality placeholder scan passed. |
| Python/JSON fixtures | Python syntax and JSON payload syntax checked. |
| Optional browser smoke | Passed isolated DOM checks for graph/arrows, evidence, hostile-text escaping, focus navigation, generated deep links, breadcrumb/popstate handlers, search, violations, and errors. Fetch/history were mocked; actual network navigation, Axum serving, and live API calls were not validated. |

Machine-readable records are in `validation/static-checks.json` and
`validation/ui-smoke.json`. There are no Rust test-pass claims in those records.

## First Rust-enabled validation pass

Run from the crate root:

```bash
cargo check --all-targets
cargo test --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets
```

Resolve any compiler/test findings and commit the generated `Cargo.lock`.
Then exercise a separately installed GitNexus against a real repository:

```bash
archgraph --root /path/to/project init
# Author mappings and rules in /path/to/project/architecture.yaml.
archgraph --root /path/to/project compile --reindex
archgraph --root /path/to/project check
archgraph --root /path/to/project context app --json
archgraph --root /path/to/project serve
```

GitNexus evidence can be incomplete. A successful check means no matching
violations were found in the observed graph; it is not proof that runtime
dependencies are absent or that a refactoring preserves behavior.
