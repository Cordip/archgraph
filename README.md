# ArchGraph

[![CI](https://github.com/Cordip/archgraph/actions/workflows/ci.yml/badge.svg)](https://github.com/Cordip/archgraph/actions/workflows/ci.yml)

**Desired recursive architecture + observed code dependencies + rules + concrete evidence.**

ArchGraph is a Rust architecture compiler for one local repository. You author
`architecture.yaml`; an external GitNexus executable supplies file-level
`IMPORTS` edges. ArchGraph maps files into logical architecture nodes, checks
constraints, writes deterministic JSON IR, and presents the same semantic focus
projection to people and coding agents.

There are no fixed C4 levels. IDs such as `app.billing.domain.invoicing` encode
an arbitrary-depth hierarchy. Logical nodes may collect files from unrelated
filesystem directories. GitNexus—not ArchGraph—owns language parsing and symbol
resolution, whether the source is Python, Rust, C/C++, or another supported
language. The one exception is CSS, which GitNexus does not parse: with
`provider.css: true` ArchGraph reads stylesheets and class names itself (see
[Stylesheets and CSS classes](#stylesheets-and-css-classes)).

## Prerequisites and quick start

Building ArchGraph requires a current stable Rust toolchain and Cargo. Running
it requires GitNexus, its own runtime prerequisites, and an indexed repository.
ArchGraph itself needs no database, LLM service, frontend package manager, or
Node/npm frontend build. The following npm command installs the **external
GitNexus provider**, not an ArchGraph frontend dependency.

```bash
# Install GitNexus according to its upstream instructions.
npm install -g gitnexus@latest
cd /path/to/your-project
gitnexus analyze --index-only

# Build/install this repository's Rust binary.
cargo install --path /path/to/archgraph

archgraph init --suggest
# Edit architecture.yaml to describe your logical modules.
archgraph compile
archgraph check
archgraph context app
archgraph serve
```

Open `http://127.0.0.1:7331`. Use a validated, pinned GitNexus version in CI:
upstream output changes must pass the strict compatibility adapter before they
can be used to report a clean architecture.

**Validation status:** the MVP was first written without a Rust toolchain. It
has since been built with Rust 1.98, its test suite run, and it has been
exercised with GitNexus 1.6.12 against [zammad](https://github.com/zammad/zammad).
That run exposed and fixed a truncation bug in the GitNexus adapter and led to
configurable edge types, reason/confidence filters and coverage warnings. A
contract test runs ArchGraph against a real GitNexus index of a generated
repository: `cargo test --test gitnexus_contract -- --ignored`. Run it after
upgrading GitNexus. See
[examples/zammad/README.md](examples/zammad/README.md) and
[IMPLEMENTATION.md](IMPLEMENTATION.md).

## Architecture source

A compact example (the complete, deeper example is
[architecture.example.yaml](architecture.example.yaml)):

```yaml
version: 1
project:
  name: example-app
  root: app
  source_roots: [src]
  exclude: ["**/generated/**", "**/target/**"]
provider:
  kind: gitnexus
  command: gitnexus
  repo: null
  page_size: 5000
  edge_types: [IMPORTS]            # add CALLS, EXTENDS, IMPLEMENTS for Ruby/Rails-style code
  exclude_reasons: [markdown-link] # documentation links are reported as IMPORTS
  min_confidence: null             # e.g. 0.6 drops GitNexus name-guessing
policies:
  unassigned_files: warn
  ambiguous_mapping: error
nodes:
  app:
    title: Application
    maps: ["src/**"]
  app.api:
    title: API
    maps: ["src/api/**"]
    interfaces:
      - {name: Public API, kind: http, direction: provides, protocol: https}
  app.domain:
    description: Business rules, independent of persistence.
    maps: ["src/domain/**"]
  app.persistence:
    maps: ["src/persistence/**"]
  app.shared:
    maps: ["src/shared/**"]
  external:
    kind: external
  external.stripe:
    title: Stripe
    kind: external
edges:
  - {id: payments, from: app.api, to: external.stripe, kind: http, label: Payments API}
rules:
  - id: domain-no-persistence
    kind: deny_dependency
    from: app.domain
    to: app.persistence
    edge_types: [IMPORTS]
    include_descendants: true
  - id: domain-allowlist
    kind: allow_only
    from: app.domain
    to: [app.shared]
    edge_types: [IMPORTS]
  - id: no-module-cycles
    kind: no_cycles
    within: app
    edge_types: [IMPORTS]
  # Upper to lower. Persistence implements the domain's ports, so it sits
  # above the domain; a layer may use any layer below it.
  - id: layering
    kind: layers
    layers: [app.api, app.persistence, app.domain]
    edge_types: [IMPORTS]
```

Every dotted parent must exist, including `external` for `external.stripe`.
Node IDs use ASCII letters, digits, underscores and hyphens separated by dots.
`app.foo` is not an ancestor of `app.foobar`. External nodes cannot have file
maps; internal nodes cannot be placed under external nodes. Project root must
reference an internal node. Unknown YAML fields, missing references, invalid
globs, duplicate rule/manual-edge IDs, and zero page sizes are errors.

Paths and globs are repository-relative. Paths normalize to `/` on every
platform. `**` spans directories; `*` does not cross `/`. Mapping and exclusion
globs are compiled once. Discovery uses `ignore` with repository `.gitignore`,
`.ignore`, and `.git/info/exclude` semantics. Global Git ignore configuration is
disabled for reproducibility. Symlinks are not followed. `.git`, `.archgraph`,
and `.gitnexus` are never crawled, so compiler/provider output cannot become
source input on subsequent runs. Missing source roots are actionable errors.

A node may set `priority` (default 0). A file matched by several nodes belongs
to those with the highest priority, and among them the rules below apply. This
lets a cross-cutting node claim files a sibling's broader glob also matches,
typically tests kept next to production code (`**/__tests__/**`, `*.spec.ts`).
Without it such files are ambiguous, or, left in the app node, create false
app ↔ test-support cycles.

A file matching an ancestor chain belongs to its deepest matching node. Parents
implicitly contain descendant files for counts and projections. Unrelated
matching branches are ambiguous. The default is an error; `ambiguous_mapping:
warn` retains the ambiguity without choosing a branch or assigning the file.
Unmatched files use the configured `ignore`, `warn` (default), or `error` policy.
Warnings and excluded/unmapped provider edges remain visible in IR diagnostics.

## Actual versus desired architecture

Observed relationships have `origin: observed` and actual file-pair evidence.
Manual typed relationships have `origin: manual`, retain authored labels and
descriptions, and have **no fabricated source evidence**. They are kept separate
even when an observed relationship has the same endpoints and kind.

Rules evaluate observed relationships only:

| Rule | Meaning |
| --- | --- |
| `deny_dependency` | Reject selected observed dependencies from the source to the target subtree. |
| `allow_only` | Permit dependencies internal to the source subtree and to listed target subtrees; reject other selected outbound dependencies. |
| `no_cycles` | Project to immediate children of `within`; report each strongly connected component containing at least two children, with a suggested cut (below). |
| `layers` | `layers` lists nodes from upper to lower; an entry may be a list of peer nodes sharing a layer. Reject dependencies from a layer to any layer above it. Downward dependencies, skipping layers, dependencies between peers and dependencies involving unlisted nodes are allowed. Descendants belong to their node's layer; a node may appear in one layer only. |

A cycle report alone does not say where to intervene, so each `no_cycles`
violation also carries `layer_order` and `suggested_cuts`. The layer order puts
the component's members from upper to lower so that as few observed
dependencies as possible point upwards: a minimum-weight feedback arc set,
weighted by observation count. It is exact for up to 16 members and uses the
greedy Eades-Lin-Smyth heuristic above that. The upward dependencies are the
suggested cut. `check` and `context` show evidence for the cut and summarize the
other edges, and the UI draws cut edges dashed. On zammad's backend the cut is
44 of 202 observations, mostly `lib -> models`. It is a starting point for
review, not a verdict: the cheapest cut can be the wrong one if the authored
layering intends otherwise.

### Baseline for legacy code

A legacy repository usually violates its intended architecture from day one,
and a check that always exits 2 gets switched off. `archgraph baseline` records
every observation (rule, file pair, relation kind) behind the current
violations in `<config stem>.baseline.json` next to the config. Commit it.
From then on `check` exits 2 only for observations missing from the baseline,
lists exactly those, and reports how many accepted observations have since
disappeared. `check --no-baseline` shows everything; `--baseline PATH`
chooses another file. Regenerate the baseline deliberately after paying off
debt, never to make a failing check pass.

Moving files does not break the baseline. It records the Git commit it was
taken at, and when observations are both new and missing, `check` asks Git
which files were renamed between that commit and the working tree, committed
or not, including untracked and edited moves (Git's usual 50% similarity).
Accepted observations follow their files, and `check` reports how many did.
Git's index and object store are left untouched: the detection runs on a
temporary copy of the index. A shallow CI clone may lack the baseline commit;
`check` then warns and treats moved files as new, so fetch enough history.
Outside Git, or with a baseline written before this existed, there is no
rename matching.

On zammad, the backend cycle (202 observations) is accepted by the baseline.
Checking the variant with resolved frontend imports against it fails with
exactly one new observation for the dependency rule,
`shared/composables/useStickyHeader.ts -> apps/mobile/.../LayoutHeader.vue`.

`edge_types` defaults to `[IMPORTS]`. The dependency rules default
`include_descendants` to `true`. With `false`, source selection and listed
allow/deny targets match exact ownership nodes; dependencies internal to the
`allow_only.from` subtree remain permitted. Empty `allow_only.to: []` permits
only internal dependencies. `no_cycles` ignores self-edges at its zoom level.
An SCC is reported as a component, not misleadingly formatted as an ordered
cycle path. Each participating architecture edge has its own evidence sample.

Interfaces are descriptive metadata: only `name` and `kind` are required, and
kinds are open strings. They do not establish runtime compatibility.

## CLI reference

Global `--root PATH` and `--config PATH` work before or after a subcommand.
Without `--root`, ArchGraph walks upward to `.git` (directory or worktree file)
and errors when none is found. Relative config paths resolve against the
repository root, not the current nested working directory.

```bash
archgraph init
archgraph init --suggest
archgraph init --install-skill
archgraph init --install-skill --force

archgraph compile
archgraph compile --reindex --json
archgraph compile --config config/architecture.yaml

archgraph check
archgraph check app.billing --reindex
archgraph check app.billing --json

archgraph show
archgraph show app.billing --format text
archgraph show app.billing --format json
archgraph show app.billing --format mermaid

archgraph context app.billing
archgraph context app.billing --json --evidence-limit 50

archgraph styles
archgraph styles app.web --json

archgraph serve
archgraph serve --reindex --port 7331 --host 127.0.0.1
```

`compile`, `check`, `show`, `context`, and server startup all compile fresh and
write `.archgraph/architecture.ir.json`. No command silently reuses stale IR.
What they do reuse is the raw provider result (`.archgraph/cache/provider.json`)
while the GitNexus index is unchanged: size and modification time of its
`meta.json` and `lbug`, plus the exact queries. Configuration and file discovery
are always read fresh, so edits to `architecture.yaml` apply at once. On zammad
this takes a compile from 7 s to 0.1 s. `--no-cache` queries GitNexus anyway.
`show` defaults to the authored project root. `show` and `context` do not reindex:
run GitNexus first when code has changed. The `--reindex` convenience belongs to
`compile`, `check`, `baseline` and `serve` only. Plain `--reindex` is incremental;
`--reindex=full` forces a rebuild without the parse cache, which is needed after
changing resolver configuration such as `package.json`, `tsconfig*.json` or
workspace files (see [docs/gitnexus-limitations.md](docs/gitnexus-limitations.md)).
`serve` checks every `--refresh-seconds` (default 5; 0 serves a fixed
snapshot) whether the GitNexus index fingerprint or `architecture.yaml`
changed, recompiles in the background and swaps the new result in; the UI
follows within seconds and stays on the node it shows. Source edits reach it
through a reindex. A failed reload keeps the last good revision and is shown
in the UI. Reloads write nothing, so an auto-index service watching the
repository is not triggered by them.

`compile` succeeds even when it finds violations. `check` exit codes are:

| Exit | Meaning |
| --- | --- |
| **0** | Compilation succeeded; no matching observed architecture violations. |
| **1** | Operational, configuration, provider, compiler, or command-line usage failure. |
| **2** | Compilation succeeded; matching architecture violations exist. |

A scoped check includes violations whose actual source or target ownership is
in the requested subtree, including cross-boundary dependencies. SCC violations
retain actual affected ownership nodes so a deep check need not flag an
unaffected sibling. Unknown focus IDs are errors. Diagnostics and indexing
progress go to stderr; successful `--json` output remains valid JSON on stdout.
Failures do not emit a fake clean JSON report or replace IR with empty results.
An older IR artifact can remain after failure, but ArchGraph does not reuse it.

`init` preserves an existing YAML and skill unless `--force` is explicit. Skill
installation targets existing `.claude` and `.agents` project directories,
creating `skills/archgraph/` beneath them. When neither exists, it creates the
project-local `.agents` destination. It does not write to global agent settings.
`--force` also overwrites an existing architecture YAML with the starter.

`init --suggest` drafts the nodes from the directory layout instead of a
one-node starter. A directory with at least 2.5% of the code files (and at
least 5) becomes a node, up to three levels deep; a directory that only wraps
one other, such as `src/`, is skipped. Each node with children gets a
`no_cycles` rule, common non-code file types present in the repository are
excluded, and Ruby code switches `edge_types` to `IMPORTS, CALLS, EXTENDS,
IMPLEMENTS`. If a GitNexus index exists, the draft is compiled once and every
node is annotated with the share of files that have an observed dependency,
so weakly covered parts are visible before any rule relies on them. The draft
reflects directories, not responsibilities: rename, merge and describe the
nodes before committing it. On zammad the unedited draft already reports the
`lib` / `app` cycle.

## GitNexus compatibility boundary

`GITNEXUS_BIN` overrides `provider.command`. Both name an executable, not a shell
command with embedded arguments. Paths containing spaces are supported as one
executable value. All provider subprocesses run with repository-root cwd.
Optional `provider.repo` is passed as a separate `--repo` argument to Cypher;
when null, the flag is omitted. Reindexing is precisely:

```text
<configured executable> analyze <repository root> --index-only
<configured executable> analyze <repository root> --index-only --force --no-parse-cache   # --reindex=full
```

The adapter captures optional `--version` metadata, then probes
`MATCH (f:File) RETURN f.filePath AS path LIMIT 1`. It fetches only the
`CodeRelation` types listed in `provider.edge_types` (default `[IMPORTS]`).
Symbol-level relations such as `CALLS` are lifted to the files containing
their endpoints and grouped per file pair, kind and reason, so every row is
unique and `SKIP`/`LIMIT` paging is stable. Query output is captured in a
temporary file: GitNexus exits before draining a pipe, which truncates results
at 64 KiB. It does not access LadybugDB, fetch `/api/graph`, invoke a shell, or
load GitNexus's complete symbol graph.

### Choosing what to observe

`IMPORTS` alone is only meaningful where dependencies are explicit imports.
Validation against [zammad](https://github.com/zammad/zammad) (see
[examples/zammad](examples/zammad/)) showed why the other options exist:

- Rails autoloads constants, so `app/models/ticket.rb` has no `IMPORTS` at all.
  With only `IMPORTS`, a rule set over the Rails backend passed with zero
  violations while the layers were in fact cyclic. Add `CALLS`, `EXTENDS` and
  `IMPLEMENTS`.
- GitNexus labels guessed relations with a `reason`. `global-name-fallback`
  (confidence 0.5) and `property-dispatch` (0.7, which links `el.focus()` to
  any function named `focus`) produced every frontend violation in zammad, all
  false. Drop them with `min_confidence: 0.6` and/or `exclude_reasons`. A
  trailing `*` in `exclude_reasons` matches a prefix.
- A rule whose `edge_types` names a relation not in `provider.edge_types` is a
  configuration error, since it could never fail.

Rows are collapsed to one observation per file pair and kind (maximum
confidence, all surviving reasons). `stats` report provider rows, filtered
rows, observations and resolved observations.

### Observation coverage

Every node records `observed_file_count`: descendant files with at least one
resolved dependency. `show`, `context` and the UI display it. When a rule's
node has fewer observed files than `policies.min_observed_ratio` (default
0.5), the IR lists it in `diagnostics.low_coverage` and, by default
(`policies.low_coverage: warn`), compilation warns that a passing check there
is weak evidence. With `low_coverage: error`, `check` exits 1: unverified,
which is neither clean nor a violation. Use it in CI once the provider sees
the code well enough; on stock zammad it fails for six rule nodes. `ignore`
silences the warning. In zammad, 99% of the CoffeeScript UI
(a language GitNexus does not parse) and 96% of the Ruby GraphQL layer
(constant references GitNexus could not resolve) had no observed dependency.
Known provider gaps, their causes and workarounds are collected in
[docs/gitnexus-limitations.md](docs/gitnexus-limitations.md).

The supported Cypher stdout contract is one JSON object containing a `markdown`
string and nonnegative integer `row_count`. The Markdown must have a header and
separator row, including on empty result pages. Dependency pages require `source`
and `target`; `kind`, `confidence` and `reason` are optional. Columns are found by name,
escaped pipes are supported, null/empty confidence means absent, a missing
`kind` column means `IMPORTS`, and malformed
rows, invalid numbers, missing columns, or mismatched row counts fail closed.
Pages continue until `row_count < page_size`. Query timeout is five minutes;
indexing itself has no imposed timeout. Keep the provider index quiescent while
compiling so offset pagination describes a consistent dataset.

Provider edges with an endpoint the configuration deliberately leaves out
(excluded, outside `source_roots`, or unassigned under `unassigned_files:
ignore`) are only counted (`stats.out_of_scope_edge_count`). Other endpoints
that do not resolve, such as an in-scope file that was not discovered (stale
index, `.gitignore`) or an escaping path, are reported in
`diagnostics.provider_anomalies`. ArchGraph also asks the provider for its list
of indexed files and reports mapped files missing from it
(`diagnostics.unindexed_files`). Paged queries fail instead of looping if the
provider ignores `SKIP`. They are never silently guessed into
logical nodes. Source paths are metadata only and are not dereferenced by the
provider adapter.

## Stylesheets and CSS classes

GitNexus has no CSS grammar and drops `import './styles.css'` (see
[docs/gitnexus-limitations.md](docs/gitnexus-limitations.md)), so a frontend's
dependency on its styles is invisible to it. With `provider.css: true`,
ArchGraph reads every discovered `.css` file (with
[lightningcss](https://github.com/parcel-bundler/lightningcss)) and every
TypeScript/JavaScript file (with tree-sitter) and adds its own observations
next to the GitNexus ones:

| Kind | Reason | Confidence | From |
| --- | --- | --- | --- |
| `IMPORTS` | `css-import` | 1.0 | `import './x.css'` in a script |
| `IMPORTS` | `css-at-import` | 1.0 | `@import './x.css'` in a stylesheet |
| `USES_CLASS` | `classname-literal` | 1.0 | `className="panel"`, `class: 'panel'`, `classList.add('panel')`, same-file constants |
| `USES_CLASS` | `classname-expression` | 0.8 | a class from a branch of `a ? 'x' : 'y'`, `ok && 'x'`, `clsx(...)`, arrays joined with `' '` |
| `USES_CLASS` | `classname-markup` | 0.8 | `class="..."` inside an HTML string or template |
| `USES_CLASS` | `classname-ambiguous` | 0.5 | the class is defined by more than one repository stylesheet |

`USES_CLASS` goes from the script to every repository stylesheet that defines
the class, because stylesheets are global. List it in `provider.edge_types`
to fetch it; GitNexus is never asked for it, and listing it without
`css: true` is a configuration error. `min_confidence` and `exclude_reasons`
apply as to GitNexus relations, so `min_confidence: 0.6` drops ambiguous
classes. Package stylesheets (`import 'leaflet/dist/leaflet.css'`) are read
from the nearest `node_modules` for their class names only; they are not
architecture nodes.

`archgraph styles [NODE] [--json]` reports the classes, whole or for the
classes a node's subtree defines or uses:

- **undefined**: used, but no stylesheet defines them;
- **unused**: defined in the repository, never used;
- **possibly used dynamically**: never used literally, but a class built at
  runtime has their prefix, e.g. `` `em-${status}` `` for `.em-idle`;
- **unresolved class expressions**: classes computed from props or data
  (`className={item.status}`); any class may come from them, so an unused
  report is a lead, not proof;
- **shared**: classes used from more than one architecture node.

A class a package stylesheet defines counts as used: the package applies it
at runtime and the repository rule overrides it. Every compile warns with the
counts. [examples/lct-task3](examples/lct-task3/) shows the report on a real
React frontend.

Not covered: CSS Modules (`styles.panel`), Sass/Less, Vue/Svelte templates,
`url()` and custom-property references, and class names in `.html` files.

## Human focus UI

The embedded HTML/CSS/plain JavaScript UI includes breadcrumbs, purpose and
interfaces, children or files, directed aggregated edges, violation markers,
node search, and an evidence panel. Click a node or edge for details;
double-click an architecture node to focus, or use the explicit Open button.
Deep links use `/?focus=app.billing.domain` and browser back/forward navigation
is supported. Only the current focus level is rendered with a small SVG layout.

A non-leaf can own files directly: these appear in a **Directly owned files**
entry rather than disappearing. A manual edge naming the focus itself appears
on a **boundary** entry instead of inventing a particular source file/child.
Outside owners appear as explicitly marked cross-boundary entries. Dependencies
collapsed to the same visible node are omitted. Manual and observed edges are
visually distinguished. Relation kinds between the same two entries are drawn
as one edge (`2 kinds × 114`); its details list each kind with its evidence. Entries are laid out in rows from upper to lower
layer (`layers` in the projection, the same minimum-upward ordering used for
cycle cuts), so dependencies point down and anything pointing up stands out.
Outside entries that only depend on the focus are drawn above it, others
below. Levels with more than 60 entries fall back to a grid. Mermaid is another renderer of the same projection,
never an architecture source format.

The server defaults to loopback and exposes only:

```text
GET /api/meta
GET /api/nodes
GET /api/focus/{node_id}
GET /api/violations
GET /api/search?q=...
```

There is no arbitrary Cypher endpoint, source-file endpoint, mutation endpoint,
or authentication. Binding beyond loopback requires explicit `--host` and
exposes architecture metadata to reachable clients. Dynamic browser text uses
`textContent`; the server provides a restrictive Content Security Policy and no
cross-origin access grants.

## AI-agent refactoring loop

```bash
archgraph context app.billing
# Use GitNexus context/query/impact and inspect relevant source.
# Edit code while preserving behavior.
gitnexus analyze --index-only
archgraph check app.billing
# Also run behavioral tests and a repository-wide check when appropriate.
```

Do not declare completion while check exits 2. Exit 1 means verification failed
and must be repaired, not that the architecture is clean. Do not edit
`architecture.yaml` unless the task explicitly requests an architecture change.
The bundled [agent skill](skills/archgraph/SKILL.md) records this workflow.
Markdown context and `--json` contain the same focus state, incoming/outgoing
observed dependencies, rules, violations, concrete evidence and agent contract.

## IR and implementation

The IR has schema version 1, ordered node maps and file records, provider
metadata, observed/manual architecture edges, resolved file dependencies, rules,
violations, coverage diagnostics and counts. There are no compilation timestamps.
Samples are sorted by file pair/kind with confidence/reason tie-breakers and
capped at 20 by default; full observation counts and confidence ranges are kept.
All resolved file dependencies are retained so a leaf view remains lossless and
`context --evidence-limit 50` can provide more than the stored aggregate sample.
No symbols, source contents, CFG, or database are copied into the IR.

The library accepts an async, object-safe `CodeGraphProvider`. The compiler,
rule engine, projection, renderers and UI know nothing about GitNexus subprocess
commands. Only `src/provider/gitnexus.rs` launches them. An in-memory provider
supports independent tests; the CLI never substitutes it for failed GitNexus.

## Checking architecture in your CI

`check` is meant to gate merges. A CI job needs GitNexus, an index and the
ArchGraph binary; the index takes minutes on a large repository (about two on
zammad), so run the job where that is acceptable. Fetch enough history for the
baseline commit, or files moved since the baseline look new (see
[Baseline for legacy code](#baseline-for-legacy-code)).

```yaml
# .github/workflows/architecture.yml
name: Architecture
on: [pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0          # the baseline records a commit
      - uses: actions/setup-node@v4
        with:
          node-version: 22
      - run: npm install --global gitnexus@1.6.12
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install --locked --git https://github.com/Cordip/archgraph archgraph
      - run: gitnexus analyze --index-only .
      - run: archgraph check        # exit 2: new violations; 1: failure
```

Commit `architecture.yaml` and `architecture.baseline.json`. With
`policies.low_coverage: error` the job also fails when a rule covers code that
GitNexus barely sees. Mind the GitNexus license (below) for commercial use.

## Tests

```bash
cargo fmt --all -- --check
cargo clippy --all-targets
cargo test --all-targets
cargo test --test gitnexus_contract -- --ignored   # needs a real GitNexus
uv run --with playwright python tests/web_smoke.py  # optional browser check
```

[CI](.github/workflows/ci.yml) runs all of them, the first three and the
contract tests on Linux and Windows.

Unit and integration tests cover hierarchy validation, mapping and file
discovery, provider wrappers and failures, rule semantics, cheapest cuts,
baselines and renames, the provider cache, drafts, deterministic aggregation
and evidence, projection, context, IR round-trips and the HTTP routes,
including live reloads. They use an in-memory provider and temporary
repositories. The CLI process tests run the real binary against
`tests/fixtures/fake_gitnexus.py` and need `python3`; they are Unix-only. The
contract tests index generated TypeScript and Ruby repositories with the real
GitNexus CLI and check the assumptions the fake encodes, including that
GitNexus still ignores stylesheet imports. The browser smoke test
loads the embedded UI with mocked fetch and history. There is no production
Python component.

## Limitations and licensing

ArchGraph covers one repository per configuration and observes the GitNexus
relation kinds listed in `provider.edge_types`, plus stylesheets and CSS
classes with `provider.css: true`. Symbol/function/AST exploration
is delegated to GitNexus. Interfaces and manual relationships are descriptive,
the UI is read-only, and there is no application database or architecture
editor. Very large file-only focuses may need further authored child nodes for
comfortable navigation. Offset pagination needs a stable index during a run;
a run that sees the index change fails instead of mixing results.

**GitNexus evidence can be incomplete.** Dynamic imports, reflection, generated
code, build-system behavior and runtime dependencies may be missed. “No observed
dependency” is not proof that no runtime dependency exists. Clean architecture
checks do not replace behavioral tests or runtime verification.

ArchGraph code is [MIT-licensed](LICENSE). The supplied design identifies the
upstream GitNexus license as **PolyForm Noncommercial 1.0.0**. GitNexus remains a
separately installed executable with its own applicable license; this repository
does not copy its implementation, embed it as a library, or relicense it. Review
the terms of the provider version/deployment you use, especially for commercial
use. The replaceable provider boundary is an engineering separation, not a
legal conclusion. Upstream: https://github.com/abhigyanpatwari/GitNexus.

The supplied product specification is preserved in [DESIGN.md](DESIGN.md).
