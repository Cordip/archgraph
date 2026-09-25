# ArchGraph implementation report

## Delivery

This is a single-crate Rust implementation; the binary name is `archgraph`.
The original design is preserved in `DESIGN.md`, followed by explicit
implementation clarifications. All required MVP areas have source
implementations; there are no placeholder commands or HTTP handlers.

The first version was written without a Rust toolchain or GitNexus. It has
since been built, tested and run against a real GitNexus index of zammad, a
large legacy Rails, CoffeeScript and Vue repository. That run found an adapter
bug and several design gaps; all are fixed, and the additions they led to
(relation kinds beyond `IMPORTS`, coverage, baselines, cut suggestions and the
rest) are described below. This report describes the current state; `git log`
has the reasons for each change. Agent workflow rules and known gotchas are in
[AGENTS.md](AGENTS.md).

## Commands

| Command | Implemented behavior |
| --- | --- |
| `archgraph init` | Non-destructive starter YAML; `--suggest` drafts nodes from the directory layout with coverage comments; `--install-skill`; explicit `--force` replacement. |
| `archgraph compile` | Fresh compilation, diagnostics, deterministic persisted IR, counts, `--json`, `--reindex[=full]`. Rule violations do not make this command fail. |
| `archgraph check [NODE]` | Fresh compilation, optional subtree filtering, text or `--json`; exit 0 for no matching violations, 1 for failure (including coverage below `policies.min_observed_ratio` under `low_coverage: error`), 2 for matching violations. With a baseline file only observations missing from it count; `--baseline`, `--no-baseline`, `--reindex[=full]`. CLI usage errors also exit 1, not 2. |
| `archgraph baseline` | Accepts all current violations into `<config stem>.baseline.json`, at file-level observation granularity. |
| `archgraph show [NODE]` | Shared semantic focus projection; `--format text`, `json`, or `mermaid`. Defaults to the configured project root. |
| `archgraph context NODE` | Agent-oriented Markdown or `--json`; configurable `--evidence-limit`; interfaces, counts, coverage, dependency direction, rules, violations with suggested cuts, concrete evidence, and agent contract. |
| `archgraph serve` | Read-only UI with embedded HTML/CSS/JavaScript that recompiles when the index fingerprint or configuration changes (`--refresh-seconds`, 0 for a fixed snapshot), `--reindex[=full]`, `--host`, `--port`; loopback by default. |

`--root`, `--config` and `--no-cache` are global options and work before or
after a subcommand. `show` and `context` also compile fresh rather than
trusting a possibly stale persisted IR; raw provider results are reused only
while the GitNexus index fingerprint and the queries are unchanged. Operational
errors propagate to the top-level exit-1 handler. No command substitutes an
empty provider result after a failure. A closed stdout (`archgraph check |
head`) exits quietly with status 141.

## Implementation map and acceptance coverage

| Area | Files | Implemented acceptance criteria |
| --- | --- | --- |
| Crate/entry point | `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/error.rs` | One Rust crate; explicit error boundary; no unsafe code; no database, LLM client, language parser, or frontend build. |
| Schema/hierarchy | `src/config.rs` | Version 1 schema, project/provider/policies, arbitrary dotted nodes, node `priority` for cross-cutting mappings such as co-located tests, optional metadata/interfaces, manual typed edges, four rule variants with per-rule `edge_types`, provider `edge_types`/`exclude_reasons`/`min_confidence`, coverage policy, actionable reference/parent/glob validation. A rule observing relation kinds the provider does not query is rejected. |
| Paths/discovery | `src/paths.rs`, `src/discovery.rs` | Repository location, source roots, exclusions, repository ignore semantics, UTF-8 forward-slash paths, Windows verbatim/UNC root handling, no Git internals traversal. |
| Membership | `src/mapping.rs` | Compiled glob sets, highest priority then deepest match in an ancestor chain, unrelated-branch ambiguity, unassigned policy, deterministic diagnostics. |
| Provider abstraction | `src/provider/mod.rs` | Object-safe async `CodeGraphProvider` and injectable in-memory test provider: info, dependency edges, indexed files, index fingerprint, query identity, incremental or full reindex. |
| GitNexus boundary | `src/provider/gitnexus.rs`, `src/provider/markdown_table.rs` | Executable override, separate argv, repository cwd, optional repo argument, stdout captured in a file (GitNexus truncates piped stdout at 64 KiB), version/probe, paginated `CodeRelation` query over the configured relation kinds with reason and confidence, paginated indexed-file listing, a guard against providers that ignore `SKIP`, index fingerprint from `meta.json`/`lbug`, `--force --no-parse-cache` for full reindexing, strict JSON wrapper/table adapter, fail-closed errors. |
| Compiler/IR | `src/compiler.rs`, `src/model.rs` | Filtering by reason and confidence, membership and observed file graph resolution, out-of-scope edge counting, anomaly diagnostics, per-node mapped/observed/unindexed file counts, coverage issues, detection of an index rewritten mid-compile, observed/manual separation, aggregation, bounded deterministic evidence, rules, timestamp-free JSON, `.archgraph/architecture.ir.json`. |
| Provider cache | `src/cache.rs` | Raw provider results in `.archgraph/cache/provider.json`, keyed on index fingerprint and query identity; configuration and discovery are never cached. |
| Rules | `src/rules.rs` | `deny_dependency`, `allow_only`, immediate-child `no_cycles`, ordered `layers` with peer groups, iterative Kosaraju, self-edge omission, minimum-weight feedback arc set (exact up to 16 members, greedy above) giving the cheapest cut and the resulting layer order, SCC and per-architecture-edge concrete evidence, precise scoped violation ownership. |
| Baseline | `src/baseline.rs`, `src/vcs.rs` | Accepted violations as file-level observations with the Git commit they were taken at; comparison into new, accepted and fixed entries, following files renamed since that commit (working tree included, via a temporary Git index). |
| Draft architecture | `src/suggest.rs` | `init --suggest`: nodes from code directories, generic `no_cycles` rules, non-code exclusions, Ruby relation kinds, coverage comments. |
| Shared projection | `src/projection.rs` | Immediate children or leaf files, lossless edge projection, collapsed self-edge omission, aggregated duplicates, cross-boundary entries, dependency layers, breadcrumbs, violation and suggested-cut annotations. |
| Agent/rendering | `src/context.rs`, `src/render/*` | Markdown/JSON context, text/JSON/Mermaid focus output, deterministic concrete examples, descriptions/interfaces, incoming/outgoing dependencies, affected rules, suggested cuts, post-edit verification contract (never regenerating the baseline unasked). |
| CLI/init | `src/cli.rs` | All seven commands, fresh compilation, required exit statuses, baseline comparison, structured cycle output, summaries, reindex options, skill installation and preservation. |
| HTTP/browser | `src/server.rs`, `src/web/*` | Read-only Axum 0.8 routes, loopback default, embedded assets, SVG directed graphs laid out by dependency layer, merged parallel edges with per-kind breakdown, suggested cuts drawn dashed, breadcrumbs, click/double-click navigation, search, focus deep links, details/evidence panels, explicit violation markers, safely created DOM text. |
| Agent skill | `skills/archgraph/SKILL.md` | Context-first workflow, GitNexus symbol investigation, edit/reindex/check loop, baseline rules, no completion on exit 2, no architecture edits without explicit authorization. |
| Documentation | `README.md`, `DESIGN.md`, `AGENTS.md`, `docs/gitnexus-limitations.md`, `examples/zammad/`, `architecture.example.yaml`, `LICENSE` | Quick start, full example, all commands, desired vs observed architecture, provider limitations with evidence, a real-repository example with findings, agent rules and gotchas, provider licensing boundary. |

The safe HTTP routes are `/api/meta`, `/api/nodes`, `/api/focus/{node_id}`,
`/api/violations`, and `/api/search?q=...`. The server offers no arbitrary Cypher,
source-file content, or mutation endpoints. Responses carry a restrictive
Content Security Policy, `nosniff`, no-referrer, and no-store headers. Binding
beyond loopback requires explicit configuration and prints a warning; there is
no authentication.

## Tests

There are **92 Rust test functions**: 54 unit tests, 20 CLI process tests, 16
compiler/projection integration tests, and two opt-in contract tests against a
real GitNexus (TypeScript and Ruby). All pass with `cargo test --all-targets`
(the contract tests are `#[ignore]`d and run explicitly). CI runs everything
on Linux and, except the Unix-only CLI process tests, on Windows. Some functions check multiple configurations
or CLI invocations.

Unit coverage includes node ID validity, missing parents, segment-aware
ancestry, node/reference/version/glob validation, provider relation-kind and
confidence validation, Windows and Unix paths, repository/worktree location,
file discovery and ignore handling, priority and deepest-match mapping,
ambiguous/unassigned policies, reason/confidence filtering and collapsing of
observations, aggregation, sorted/capped evidence, all four rules, SCC
behavior, cheapest-cut optimality against brute force, provider wrapper
compatibility, escaped-pipe Markdown parsing, malformed rows, the provider
cache, draft generation, CLI argument/exit semantics, skill preservation, and
Mermaid label escaping.

`tests/compiler_projection.rs` adds in-memory provider compilation, provider
failure propagation, IR serialization/persistence, repeated/shuffled-input
determinism, an index rewritten mid-compile, cached versus fresh provider
results, unmappable provider-edge diagnostics, semantic projection, dependency
layers, cross-boundary/manual edges, direct ownership, leaf graph preservation
beyond the 20-example aggregate cap, larger context evidence limits,
actual-owner scoped checks, and read-only Axum route tests.

`tests/cli.rs` adds Unix process-level checks for exits 0/1/2, invalid arguments,
configuration/provider failures, JSON output, Mermaid, pagination termination
and a provider that ignores `SKIP`, output larger than a pipe buffer from a
Node-like provider, a closed stdout, repository cwd, spaced
executable/repository arguments, incremental and full reindexing without
corrupting JSON stdout, baselines, the coverage policy, unindexed and
out-of-scope files, the provider cache, `init --suggest`, and cycle output.
These tests use the explicitly test-only `tests/fixtures/fake_gitnexus.py`
executable and require Python 3. Core Rust tests do not require GitNexus or
Python. Production ArchGraph is Rust and embedded JavaScript, not Python.

`tests/gitnexus_contract.rs` indexes a generated 500-feature TypeScript
repository with the real GitNexus CLI under an isolated `HOME` and checks the
assumptions the fake provider encodes: output size, relation kinds, Markdown
link filtering, and a stable index fingerprint.

The separate optional `tests/web_smoke.py` harness exercises the embedded
browser code in Chromium with mocked fetch and history interfaces, including
merged edges and the layered layout. It is not part of `cargo test` and not a
production dependency.

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
8. **Relation kinds beyond `IMPORTS`.** The design observed imports only. On
   Rails code that reports a clean architecture, because autoloaded constants
   are never imported, so `provider.edge_types` selects `CALLS`, `EXTENDS` and
   `IMPLEMENTS` as well. Guessed relations are filtered by
   `provider.exclude_reasons` and `provider.min_confidence`.
9. **Coverage is part of the result.** A file with no observed dependency, or
   missing from the index altogether, cannot violate a rule. Nodes carry
   observed and unindexed file counts, and `policies.low_coverage: error`
   makes `check` fail instead of passing on unverified code.
10. **Baselines.** Legacy repositories start with violations. A baseline
    accepts the current file-level observations so `check` fails only on new
    ones; regenerating it is a deliberate act, never an agent's default.

No required command, rule, provider adapter, context surface, or UI feature was
intentionally omitted.

## Validation performed

| Validation | Status and scope |
| --- | --- |
| Rust compilation, tests, rustfmt, Clippy | Rust 1.98 on Linux: all tests pass, rustfmt clean, no Clippy warnings. |
| Windows | Rust 1.97 (MSVC) on Windows 11: unit, integration and both real-GitNexus contract tests pass (GitNexus 1.6.12 launched through `gitnexus.cmd`). The CLI process tests are Unix-only. |
| Real GitNexus | GitNexus 1.6.12 against zammad (about 14,000 files) with the configuration in [examples/zammad](examples/zammad/), plus the contract test above. Findings about zammad and about GitNexus are in [examples/zammad/README.md](examples/zammad/README.md) and [docs/gitnexus-limitations.md](docs/gitnexus-limitations.md). Three consecutive compiles produce byte-identical IR, cached or not. |
| `node --check src/web/app.js` | Passes. |
| Browser | `tests/web_smoke.py` passes in Playwright Chromium; `archgraph serve` on zammad renders with no console errors and was inspected by screenshot. |

`validation/static-checks.json` and `validation/ui-smoke.json` are records of
the original static review, made before any build; they are kept for history
and make no claims about the current code.

## Validating a change

The commands and the real-repository check every change must pass are listed
under **Workflow** in [AGENTS.md](AGENTS.md).

GitNexus evidence can be incomplete. A successful check means no matching
violations were found in the observed graph; it is not proof that runtime
dependencies are absent or that a refactoring preserves behavior.
