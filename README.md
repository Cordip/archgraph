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
language. The exceptions are three gaps GitNexus leaves in a web
application: CSS, which it does not parse; HTTP calls from a frontend, which
it links to backend routes only for `fetch('/literal')`; and imports of
third-party packages, which it drops. With `provider.css: true`,
`provider.http: true` and `provider.packages: true`, ArchGraph reads
stylesheets, class names, client HTTP calls and import statements itself
(see [Stylesheets and CSS classes](#stylesheets-and-css-classes),
[HTTP calls between frontend and backend](#http-calls-between-frontend-and-backend)
and [Imported packages](#imported-packages)).

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

## Where the architecture file lives

ArchGraph supports two ways of keeping `architecture.yaml`:

- **In the project (the default).** The file sits at the project's root and
  is committed with the code, so the architecture is reviewed and versioned
  like any other change. Commands run from the project need no options:
  `archgraph check`, `archgraph serve`.
- **Outside the project (development).** While the architecture is being
  worked out, or when ArchGraph should leave no trace in a repository, the
  file lives in another repository, for example this one
  (`examples/lct-task3/architecture.yaml` describes lct-task3), and every
  command names both:

  ```bash
  archgraph --root /path/to/project \
      --config /path/to/archgraph/examples/project/architecture.yaml check
  ```

  Give `--config` as an absolute path: a relative one is resolved against
  `--root`. The project then only receives `.gitnexus/` from GitNexus and
  `.archgraph/` (the compiled IR and the provider cache), which ignores
  itself, so the project's Git status stays clean.

Moving from the second mode to the first is copying the file into the
project and committing it.

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
reference an internal node. External nodes may map imported packages
(`package:` globs, see [Imported packages](#imported-packages)). Unknown YAML
fields, missing references, invalid
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
| `allow_only_from` | The inverse: permit dependencies on the `to` subtree only from listed `from` subtrees and from inside `to`; reject every other selected inbound dependency. "Only the planning core may use the solver." An empty `from` lets nothing outside use `to`. |
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
allow/deny targets match exact ownership nodes (for `allow_only_from`, the
target and the listed sources); dependencies internal to the
`allow_only.from` subtree, or to the `allow_only_from.to` subtree, remain
permitted. Empty `allow_only.to: []` permits
only internal dependencies. `no_cycles` ignores self-edges at its zoom level.
An SCC is reported as a component, not misleadingly formatted as an ordered
cycle path. Each participating architecture edge has its own evidence sample.

Interfaces are descriptive metadata: only `name` and `kind` are required, and
kinds are open strings. They do not establish runtime compatibility.

## Snapshots: what a refactoring changed

A refactoring moves files and cuts dependencies over many steps, and a
check only says where it stands now. `archgraph snapshot [--name NAME]`
compiles fresh and saves the result in `.archgraph/snapshots/NAME.json`
(default `before`), with the Git commit it was taken at; a snapshot of the
same name is replaced. Like everything in `.archgraph/`, it is never
committed and leaves the repository's Git status clean, so it works in both
modes of [Where the architecture file lives](#where-the-architecture-file-lives).

`archgraph diff [NODE] [--snapshot NAME] [--json]` compiles fresh and
reports what changed since, in the whole architecture or the subtree of
`NODE`:

- violation observations (rule and file pair, as in a baseline) that
  appeared and that were resolved;
- file dependencies added and removed, with the nodes at both ends;
- files added, removed and moved, and files the architecture now assigns to
  another node (an edit of `maps`).

A file moved since the snapshot stays one file: as with a baseline, Git is
asked which files were renamed since the snapshot's commit, committed or
not, and the snapshot's paths follow them, so a moved file's dependencies
and violations are not reported as removed and added again. Without Git,
or when Git fails (a warning says so), a move is a removed and an added
file. `diff` is a report and exits 0; it changes nothing.

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

archgraph http
archgraph http app.web --json

archgraph packages
archgraph packages ortools --json

archgraph unused
archgraph unused app.web --json

archgraph snapshot
archgraph snapshot --name before-split
archgraph diff
archgraph diff app.billing --snapshot before-split --json

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
`.archgraph/` holds only generated files and ignores itself with a
`.gitignore` of `*`, as GitNexus does for `.gitnexus/`, so the analyzed
repository's Git status stays clean. An existing `.gitignore` there is kept.
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

## HTTP calls between frontend and backend

A frontend and its backend share no imports; they meet over HTTP. GitNexus
finds server routes (`Route` nodes, e.g. from FastAPI decorators, with the
method and the handling file) and links a client to a route only for
`fetch('/literal')` and a few wrapper names such as `apiFetch`. A frontend that
calls `request('/plans')`, with `request` adding a base URL, is invisible to it
(see [docs/gitnexus-limitations.md](docs/gitnexus-limitations.md)).

With `provider.http: true` and `FETCHES` in `provider.edge_types`, ArchGraph
lists the routes from GitNexus, reads client calls in TypeScript/JavaScript
with tree-sitter and adds `FETCHES` from the calling file to the file handling
the matched route, next to GitNexus's own `FETCHES`:

| Reason | Confidence | Call |
| --- | --- | --- |
| `http-call` | 1.0 | `fetch`, `new EventSource`, `new WebSocket`, `axios.<verb>`, `navigator.sendBeacon` |
| `http-wrapper` | 1.0 | a same-file function passing its parameter into such a call, e.g. `request('/plans', { method: 'POST' })` |
| `http-link` | 0.8 | a string built on a call's base constant outside any call, e.g. a download link `` `${BASE}/plans/${id}/export` `` |

URLs are evaluated through the file's string constants; any other value is a
path parameter. A call matches the route with the most literal segments that
accepts its method; a route of parameters only, such as a single-page app's
catch-all `/{path:path}`, matches nothing. Rules over `FETCHES` then express
the API boundary, e.g. `deny_dependency` from UI components to the backend so
that only the HTTP client module calls it.

`archgraph http [NODE] [--json]` lists every route with the calls reaching it,
calls that reach no route or use a method the route does not accept (a broken
contract), calls to absolute URLs of other services, and calls whose URL is
not known statically. Every compile warns with the counts.

Not covered: server routes GitNexus does not report (check the route list
of `archgraph http` against the backend), calls through wrappers defined in
another file, GraphQL and other non-path protocols, and calls made by Python
or other backend code.

## Imported packages

GitNexus keeps an `IMPORTS` edge only when an import resolves to a
repository file; `from ortools.constraint_solver import pywrapcp` or
`import L from 'leaflet'` leaves no trace in its index (see
[docs/gitnexus-limitations.md](docs/gitnexus-limitations.md)), so it cannot
say who uses OR-Tools. With `provider.packages: true`, ArchGraph reads the
import statements of every discovered Python and TypeScript/JavaScript file
with tree-sitter and adds an `IMPORTS` edge from the importing file to the
package:

| Reason | Confidence | Import |
| --- | --- | --- |
| `package-import` | 1.0 | `import a.b`, `from a.b import c`, `importlib.import_module('a')`, `__import__('a')`; `import … from 'x'`, `import 'x'`, `export … from 'x'`, `import x = require('x')`, `import('x')`, `require('x')` |
| `package-type-import` | 1.0 | `import type … from 'x'`, `export type … from 'x'`, and Python imports under `if TYPE_CHECKING:` |

The edge's target is the package's pseudo-path `package:python/<top-level
module>` or `package:npm/<name>`: `from ortools.constraint_solver import
pywrapcp` imports `package:python/ortools`, `import
'leaflet/dist/leaflet.css'` imports `package:npm/leaflet`, and
`@tanstack/react-query/devtools` imports `package:npm/@tanstack/react-query`.
The ecosystem is part of the name because one name can be two libraries
(`yaml` in Python and in npm). `import { type A } from 'x'` still counts as
a runtime import: under `verbatimModuleSyntax` it loads `x`.

**Local or package.** An import becomes a package only when it is clearly
not local. ArchGraph decides from the repository's own files, walked like
discovery but regardless of `exclude` and `source_roots` (an excluded module
is still local):

- Python: relative imports are local. An absolute import is local when its
  top-level module (`name.py`, or a directory holding Python files) sits
  beside the importing file, in a directory above it, or in a `src/`
  directory there: the roots a script or test runner started there imports
  from. A standard-library module is no package. A top-level name the
  repository defines only somewhere else, such as `import redis` in
  `app/cache.py` next to `tools/redis.py`, is ambiguous. Anything else is a
  package. (GitNexus links that `import redis` to `tools/redis.py` by a
  repository-wide name match; ArchGraph does not follow its Python
  resolution.)
- TypeScript/JavaScript: relative, absolute and `#` subpath specifiers are
  local, and so is a specifier that cannot be an npm package name (`@/x`,
  `~/x`, `$lib/x`: path aliases). The `name` of any `package.json` in the
  repository is a local workspace package. Otherwise a bare specifier is a
  package when a `package.json` beside the importing file or above it
  declares it, or its `@types/` package, in `dependencies`,
  `devDependencies`, `peerDependencies` or `optionalDependencies`. An
  undeclared specifier is local when GitNexus resolved it to a repository
  file (a tsconfig path alias: `@app/utils/date` to
  `src/app/utils/date.ts`), no package when it is a Node.js built-in (`fs`,
  `node:fs`), and ambiguous otherwise: an alias GitNexus did not resolve, or
  a package used without being declared. `virtual:` and other scheme
  specifiers are ambiguous too.

Ambiguous imports and dynamic imports of a computed module (`import(name)`)
are never observed. Every compile warns with their counts, and `archgraph
packages` lists them with the reason.

**Packages in the architecture.** Packages are not files: they count in no
file total, never make a file unassigned, and importing one does not make a
file observed for coverage (a file whose only dependencies are packages
still tells nothing about the repository-internal edges a rule checks).
Each package belongs to one node, which owns it the way a node owns files:

- An external node may map packages with `package:` globs, e.g.
  `maps: ["package:python/ortools"]`. `*` does not cross `/`, so
  `package:npm/@tanstack/*` maps a scope and `package:npm/**` every npm
  package; `package:*/yaml` matches both ecosystems. `priority` and the
  deepest-match rule apply as for files. Internal nodes cannot map packages
  and external nodes cannot map files. A package glob that matches no
  imported package is a warning, since a rule about it could never fail.
- Every package no node maps belongs to `packages`, "External packages",
  which ArchGraph adds as an external root node. Define `packages` yourself
  (it must be external) to retitle it or to put package nodes below it; a
  node of that name that is not external is an error.

Rules work on package nodes unchanged, since a package import is an
ordinary `IMPORTS` observation:

```yaml
nodes:
  libs: {kind: external, title: Third-party libraries}
  libs.python: {kind: external, title: Python packages, maps: ["package:python/**"]}
  libs.python.ortools: {kind: external, title: OR-Tools, maps: ["package:python/ortools"]}
rules:
  # Only the planning core (and its tests) may use the solver.
  - {id: only-the-core-solves, kind: allow_only_from, to: libs.python.ortools, from: [lct.backend.core, lct.tests]}
  # The frontend imports no Python package.
  - {id: frontend-imports-no-python, kind: deny_dependency, from: lct.frontend, to: libs.python}
```

Here `ortools` belongs to `libs.python.ortools` (the deepest match), every
other Python package to `libs.python`, and npm packages to `packages`.

`allow_only` restricts packages too: its source may depend only on its
targets, so list `packages` or the package nodes it may use in `to`.
`no_cycles` and `layers` are unaffected unless their nodes include package
nodes. `provider.packages` needs `IMPORTS` in `provider.edge_types`;
`exclude_reasons: [package-type-import]` drops type-only imports. The IR
lists every package with its node and every import (`packages` in the IR,
`packages` and `descendant_package_count` on nodes); without
`provider.packages` none of these fields appear.

**Where they show.** In `show`, `context` and the UI the owning node is an
entry like any external node: "External packages" outside the focus, with
an edge from each importing node. At the owner's own level each package is
an entry of its own, whose details list every importing file and line with
its node. `context` also lists the packages a subtree imports.
`archgraph packages [PACKAGE] [--json]` prints every package with its node,
the importing nodes and each `file:line`, then the ambiguous and dynamic
imports; `archgraph packages ortools` shows one package (in both ecosystems
if both have the name; `python/ortools` picks one).

Not covered: Ruby (Rails and Bundler load gems without an import statement,
and `require` is not read either), `.vue` and `.svelte` single-file
components, CoffeeScript, versions, and runtime loading through entry points
or plugin registries. A Python package is named by its top-level module, not
its distribution: PyYAML is `yaml`, and every `google.cloud.*` library is
`google`. A local module that exists only after a build or is ignored by Git
looks like a package.

## Entry points and files with no observed users

Some files are loaded by name, not imported: Vite reads
`frontend/vite.config.ts`, `index.html` loads `frontend/src/main.tsx`,
pytest collects `test_*.py`, uvicorn starts `planner.api.app:app` from a
string. Nothing observed depends on them, and neither does anything depend
on dead code. ArchGraph tells the two apart only if the configuration says
which files are entry points:

```yaml
project:
  entry_points:
    - frontend/vite.config.ts
    - frontend/src/main.tsx
    - "backend/tests/**/test_*.py"
```

Entry points are globs over repository files, matched against mapped files;
`package:` globs are an error. An entry that matches no mapped file is a
warning.

Every mapped file then gets a `usage` in the IR:

| Usage | Meaning |
| --- | --- |
| `entry_point` | Declared in `project.entry_points`. |
| `used` | Another file has an observed dependency on it. |
| `no_observed_users` | Not declared, and nothing observed depends on it. |
| `not_indexed` | Not in the provider's index (`diagnostics.unindexed_files`): unknown. |

Observed dependencies are the `provider.edge_types` that survive
`exclude_reasons` and `min_confidence`, including ArchGraph's own CSS, HTTP
and package observations. A file's dependency on itself does not count, and
neither does an import of a package: packages are not files. A user may be
outside the architecture: a script outside `source_roots`, an excluded or an
unassigned file still uses what it imports. An observed user wins over "not
indexed", since ArchGraph's CSS edges reach stylesheets GitNexus does not
index.

`no_observed_users` is a dead-code **candidate, never a proof**. GitNexus
does not see Ruby autoloading, HTML script tags, dynamic imports, framework
conventions (Rails controllers, pytest fixtures in `conftest.py`) or anything
loaded by a string (see
[docs/gitnexus-limitations.md](docs/gitnexus-limitations.md)). Read the file
and search for its name before deleting it; declare it as an entry point if
a tool loads it.

Each node counts its descendant `entry_point_count` and
`no_observed_users_count`, and `outside_user_count`: distinct files outside
its subtree with an observed dependency on a file inside it. A node that is
not top-level, has indexed files, declares no entry point and has no outside
user may be unused as a whole.

`archgraph unused [NODE] [--json]` lists the files with no observed users
by node (entry points and unindexed files left out), the topmost nodes that
nothing outside uses, the number of declared entry points and unindexed
files, and the coverage of the scope: where few files have any observed
dependency, most of the list is unseen rather than unused. It exits 0; it is
a report, not a check. `context` gives the counts and the first 20 files of
its subtree, and the UI marks entry points and files with no observed users
(see [Human focus UI](#human-focus-ui)).

## Human focus UI

The embedded HTML/CSS/plain JavaScript UI works offline (no external fonts or
scripts) and follows the system light or dark mode. The current level is an
infinite canvas, drawn like a drawing sheet:

- **Canvas.** Drag empty space (or anything while holding Space) to pan; use
  the wheel or a pinch to zoom at the pointer, a two-finger scroll to pan.
  Drag an entry to move it: while it moves, the drawing is dimmed and its
  wires take a simple course (a straight line, or one elbow or a Z on a
  board); the drop lays the level out again and Escape cancels the move.
  Positions are remembered per focus in the browser, and **Reset layout**
  puts everything back. **Fit**, **Selection**
  and the zoom percentage are in the bottom-right corner with a minimap.
  Double-click an architecture node (or Shift+Enter) to open it: the view
  dives into the card; going up zooms back out of it.
- **Floating tools** in the top-left corner: search (`/`; entries of the
  current level first, then architecture nodes anywhere, then imported
  packages by name) and **Filters**:
  only violations or only files with no observed users (the rest is
  dimmed), relation kinds, observed and manual dependencies, and entries
  outside the focus.
- **Compare** (next to Filters, off by default) draws the live graph
  against a snapshot saved with `archgraph snapshot` (see [Snapshots: what
  a refactoring changed](#snapshots-what-a-refactoring-changed)). It lists
  the saved snapshots, or says how to take one when there are none; the
  choice is remembered in the browser and in the address
  (`/?focus=app&compare=before` opens with it on). While it is on, the
  level's comparison (`/api/diff`) is fetched with each level and each new
  live revision, and changes are drawn as revisions, never by colour
  alone: a changed entry carries a tag on its top right corner (`+ new`,
  `↦ moved` for a renamed file, `▲ 3→5 files`, or for a directory group
  how many of its files are new or moved), a new wire runs between two
  rails of the revision ink with a `+ new` tag, and a wire whose count
  changed has a `▲`/`▼ before→after` tag (a trunk's tag counts its changed
  wires, `Δ n`). What is gone is drawn in grey phantom lines: a gone wire
  between entries still drawn (or a gone entry) is a dashed line with an
  open arrowhead and a `− count` tag, and a gone entry is a hatched ghost
  card in a band below the level (the first 40; all of them are listed in
  the details). The legend explains the marks present, and pointing at a
  row lights them. The level's details get a "Since snapshot" section:
  counts of entries, dependencies and violations that changed, the
  violations that appeared (each opening its evidence) and were resolved,
  the entries that are gone, and the subtree's files moved, added, removed
  and assigned to another node. A selected entry, wire or ghost says how
  it changed. **Only changes** dims everything unchanged. A level that did
  not exist in the snapshot is not marked entry by entry; its legend and
  details say that everything on it is new. A snapshot that cannot be read
  is shown as an error, not as a comparison without changes.
- **Node list** on the left: the whole architecture tree with the number of
  violations in each subtree, an "Only with violations" switch, and the list
  of all violations. Clicking a node selects and centres it in its parent's
  level; clicking a violation opens the level where it happens.
- **Details** on the right: with nothing selected, the level itself (purpose,
  interfaces, counts, violations in view and coverage diagnostics); otherwise
  the selected entry, edge or violation with its evidence. A node's details
  list what it depends on and what uses it, each opening that dependency's
  evidence. Both side panels collapse; on narrow screens they are drawers.
  On wider screens the right panel (details and source together) is resized
  by dragging its left edge, or with the arrow keys on the focused edge
  (Shift for bigger steps, Home and End for the narrowest and widest,
  Enter or a double-click for the default). A drag moves only the edge and
  applies the width on release (Escape cancels it), since resizing the
  canvas redraws a large level. The panel is at least 300 px wide and the
  canvas keeps at least 320 px; the width with the source open and the
  width without it are remembered separately in the browser.
- **Source** opens below the details when a file is selected (a card, a
  file in a list or the table) or an evidence line is clicked: the file with
  line numbers and light colouring for Rust, TypeScript/JavaScript, Python,
  Ruby, CSS, Vue components (tags in the template, TypeScript in
  `<script>`, CSS in `<style>`) and ERB (Ruby inside `<% %>`, tags
  outside), its path with a copy button, and a divider between details
  and source whose position is remembered. The inspector widens while it is
  open, and the canvas keeps what it showed in place: the selected entry or
  wire stays where it was in the visible canvas (the view's centre stays the
  centre when nothing selected is in view), as it does when a side panel is
  shown or hidden or the window is resized. Closing the viewer, or clearing
  the selection (Escape, **Back to this level**), closes it. An evidence line scrolls to the lines it is about and
  marks them, with a note saying how they were found:
  - an import of a package (`provider.packages`) has its exact line;
  - a dependency between files has none, since GitNexus reports files and
    not lines, so the viewer searches the source file's text: first import
    lines whose module path ends with the target's (`../domain/model`,
    `domain.model`, `crate::domain::model`, a directory for its `index` or
    `__init__`), otherwise lines naming the target's file name or its
    CamelCase as a whole word (`ldap_source` or `LdapSource`, as Rails code
    refers to a file). These are marked as found in the text; a `CALLS`
    edge through a symbol with another name has no line to show, and the
    note says so.

  Only the rows in view are drawn, so a 12,000-line file opens in about
  60 ms. With a live server an open file is reloaded with each new revision,
  and marked stale, with a **Reload** button, when it changes on disk in
  between. The viewer asks the server only for repository-relative paths
  (see the endpoint's scope below) and shows its refusals.
- **Table** lists the level's entries file by file, grouped by directory, with
  dependency counts, a filter and other orders.
- **Matrix** is a dependency structure matrix of the level: one row and one
  column per entry, in the same order on both axes (the canvas's bands,
  then its layers from upper to lower, by name within each, with a header
  band for each), and in each cell the number of dependencies of the row's
  entry on the column's. The tooltip lists the relation kinds; a violating
  cell is red, and a cell below the diagonal (a dependency against the
  layer order) has an amber corner. It uses the canvas's filters and
  directory groups (double-click a group row to expand it). Hovering a
  cell lights its row and column and the matching rows of the details
  panel; clicking it shows the evidence. The matrix scrolls in its own
  pane with sticky headers, so levels of a thousand entries stay usable.

Selecting a node, edge or violation dims everything it is not connected to.
Entries that take part in a violation get a red revision cloud, and each
architecture card shows its observed share as a bar. The evidence notice stays
visible at the bottom of the canvas. The canvas is one tab stop: arrow keys
move between entries, Enter shows details, Shift+Enter opens, `+`/`-` zoom,
Shift+1 fits and Shift+2 zooms to the selection. Deep links use
`/?focus=app.billing.domain` and browser back/forward navigation is supported.

With `provider.packages`, the node owning packages ("External packages"
unless mapped otherwise) is an external card showing how many packages it
holds. At its own level every package is a card of its own, drawn with round
corners and a crate glyph; its details list every importing file and line,
grouped by the importing node, and selecting a package from search opens
that level with the package selected: the quickest answer to "who uses
OR-Tools".

**Usage** (see [Entry points and files with no observed
users](#entry-points-and-files-with-no-observed-users)) is drawn calmly, since
it is not a violation. A declared entry point carries a green start glyph and
"entry point" on its card. A file nothing observed depends on carries a
dashed violet ring and "no observed users"; its details say it may be an
entry point for a tool or dead code, and to declare it in
`project.entry_points` if it is an entry point. An unindexed file reads "not
indexed": unknown. A node that nothing outside uses gets the ring too; a node
with some such files shows the ring with their number, and its details offer
**Show files with no observed users**, which opens it with that filter on.
Search results, the table (a Usage column, "No observed users first") and
the level's facts show the same status.

**Wires.** The tools next to Filters choose how dependencies are drawn:

- **Curves** (the default): smooth curves between rows, as described below.
- **PCB**: a circuit board. Cards sit on a grid; traces run only
  horizontally, vertically or at 45° (chamfered corners), each on its own
  track at a 12 px pitch in the channels between rows and columns, ordered
  to avoid crossings. Ports are spread along the card edges, a channel
  widens when it needs more tracks, and where a crossing is unavoidable the
  horizontal trace hops over the other; every trace also cuts a small gap
  into the traces it crosses. A via marks where each trace starts.
- **Hex**: a hexagonal board. Cards are hexagonal plates (the text stays
  horizontal) in rows offset by half a slot, like honeycomb cells; traces run
  at 0°, 60° and 120° along the horizontal channels and through the 60° gaps
  between plates, with the same tracks, hops and gaps. A level where more
  than 40 traces would cross one gap is drawn as the PCB board instead, and
  the legend says so.

**Colour** gives each wire a colour: by **source** (the default: every entry
with outgoing dependencies is a net, and all its wires share its colour,
also shown as a tab on its card), by **relation kind** (an edge of several
kinds is drawn as a bus of strands, one per kind), by **target**, or
**none**. The six hues are the colour-blind-safe Okabe–Ito set, tuned for
the light sheet and the dark blueprint; past six nets they repeat with a
dash pattern (dashes of four lengths, never dots). Violations keep a red casing around the wire's colour and
suggested cuts stay dashed. A legend in the top-right corner lists the
colours (hovering a row lights its wires), and the details panel shows each
dependency's wire sample. The mode and colouring are remembered in the
browser; dragging a card on a board drops it into the nearest grid cell
(swapping with the card there) and re-routes, and each mode remembers its
own positions. The same input always gives the same routing.

**Trunks.** Wires from several sibling entries (those inside the focus, or
those outside it) into the same target merge into a trunk, like a cable
harness, once three or more head the same way. The trunk is a ribbon cable:
one thin strand per colour among its wires, side by side (per source
colour, or per relation kind when colouring by kind), and each source's
branch runs into its own strand. Past six colours, sources that share a hue
share its strand (drawn solid), and any rest go into a grey strand. When
colouring by target, or without colours, a trunk is one wide strand. It ends
in one arrowhead as wide as the ribbon at the target, chevrons along it
point the way, and a tag beside the arrowhead names the count and the
target (`×27 → External packages`); zoomed out, the tag shows only the count
and grows so it stays readable. Hovering a trunk lights its wires and their
entries, and clicking it lists them, each opening its evidence; with one of
its wires selected, that wire's strand shows through the dimmed ribbon. If
any of its wires violates a rule, the red casing surrounds the whole ribbon
and the tag gives the number of violating wires. The boards keep their
angles and track pitch: a trunk's wires share one port, one channel and a
band of tracks per channel wide enough for the ribbon. Manual dependencies
never merge.

**Focus** (the button next to Colour, on by default): on a level with more
than 60 drawn wires, the other wires are drawn lighter and solid (their dash
patterns return when they are lit) until an entry, a wire or a legend row is
pointed at or selected, and then only its wires are drawn at full strength.
Trunks, violations and every arrowhead stay at full strength, and each wire
turns solid and full-coloured for its last stretch before the arrowhead, so
the direction reads without pointing. Arrowheads carry a thin casing of the
sheet's colour and keep at least 11 px on screen.
The setting is remembered with the mode and colouring.

**Zoomed out.** Wires keep at least 1.5 px on screen, trunk strands 2 to 3
px (a ribbon at most about 12 px), and arrowheads at least 9 px, so wires,
their colours and their directions stay readable at 20%. The widths change
in 10% steps of the zoom.

**Large levels.** Only what is in view, and a margin around it, is in the
drawing: cards, wires and trunks further away join it when the view settles
after a pan or zoom, and search, the minimap, Fit and highlighting still
reach them. The drawing extends half a canvas beyond each edge, so a long
pan shows no blank strip. With focus mode on, the dimmed wires are drawn
together as a few paths, one per colour and weight; pointing at one still
lights it.

**Text size.** Card titles and wire labels keep a readable size on screen:
below 100% they grow as the view zooms out, up to what a card can hold, and
are refitted so that long names never overflow. Below 60% a card shows only
its title, on up to two lines, and wire labels appear only on highlighted
wires.

Levels with more than 120 entries (zammad's leaves have up to 2,577 files)
show their files as **directory groups**, about 30 per level: double-click a
group (or its + button) to expand it in place into subdirectories and files,
and collapse it again from the details of anything inside it.

A non-leaf can own files directly: these appear in a **Directly owned files**
entry rather than disappearing. A manual edge naming the focus itself appears
on a **boundary** entry instead of inventing a particular source file/child.
Outside owners appear as explicitly marked cross-boundary entries. Dependencies
collapsed to the same visible node are omitted. Manual and observed edges are
visually distinguished. Relation kinds between the same two entries are drawn
as one edge (`2 kinds × 114`); its details list each kind with its evidence. Entries are laid out in rows from upper to lower
layer (`layers` in the projection, the same minimum-upward ordering used for
cycle cuts), so dependencies point down and anything pointing up stands out;
within a row, entries are ordered to reduce crossings, and edges that skip
rows pass between the cards of the rows in between. The focus is drawn as a
frame; outside entries that only depend on it are drawn above the frame,
others below. Levels with more than 60 entries, and directory groups, have
no layers from the server; the UI layers them itself (cycles broken in entry
order, then longest paths), with unconnected entries in a last row. Above 40
edges, labels appear only on highlighted edges. Work planned for the UI is
listed in [docs/ui-backlog.md](docs/ui-backlog.md).
Mermaid is another renderer of the same projection,
never an architecture source format.

The server defaults to loopback and exposes only:

```text
GET /api/meta
GET /api/nodes
GET /api/focus/{node_id}
GET /api/violations
GET /api/search?q=...
GET /api/packages
GET /api/source?path=<repository-relative path>[&if_hash=<hash>]
GET /api/snapshots
GET /api/diff/{node_id}[?snapshot=<name>]
```

`/api/diff` compares one level with the same level of a saved snapshot
(default `before`): entries added, removed, moved (a file renamed since the
snapshot) and resized; edges added, removed and with another count;
violations that appeared and were resolved; and the subtree's file-level
diff as `archgraph diff` reports it. A snapshot name is a file name of
letters, digits, `-`, `_` and `.`, not starting with a dot (`invalid`
otherwise), and a missing snapshot or node is refused (`missing`,
`unknown_node`). The parsed snapshot is kept while its file and the live
revision are unchanged.

`/api/source` is the only endpoint that reads source files (`/api/diff` reads
only `.archgraph/snapshots/` and asks Git about renames), and its scope
is narrow. It serves a file only when the architecture being served maps it
to a node (files that are excluded, outside `source_roots`, unassigned or
ambiguous are refused, whatever is on disk), and only as UTF-8 text. The
checks run in this order, and a refusal carries an HTTP status, a `reason`
code and a message:

| Refused | Status | `reason` |
| --- | --- | --- |
| no path | 400 | `empty` |
| an absolute path (`/…`, `\…`, `C:…`) | 400 | `absolute` |
| a `..` segment | 400 | `parent` |
| any other spelling than the IR's (`./`, `//`, backslashes) | 400 | `not_normalized` / `invalid` |
| a file the IR does not map to a node | 403 | `unmapped` |
| a file deleted since the compile | 404 | `missing` |
| a file that is itself a symlink | 403 | `symlink` |
| a path that resolves outside the canonical repository root | 403 | `outside` |
| a directory or special file | 403 | `not_file` |
| more than 1 MiB (1,048,576 bytes; at most one byte more is read) | 413 | `too_large` |
| content with a NUL byte | 415 | `binary` |
| content that is not valid UTF-8 | 415 | `not_utf8` |

A served file comes with its node, `line_count`, `bytes` and `hash` (FNV-1a
64 of the bytes, for telling a changed file from an unchanged one; not a
security hash). With `if_hash` equal to the current hash the text is left
out and `unchanged` is true. A server built from a fixed snapshot without a
repository root (`server::router`) refuses every request (`disabled`).

There is no arbitrary Cypher endpoint, no way to read an unmapped file, no
mutation endpoint and no authentication. Binding beyond loopback requires
explicit `--host` and exposes architecture metadata, and the source of every
mapped file, to reachable clients. Dynamic browser text uses
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
The bundled [agent skill](skills/archgraph/SKILL.md) records this workflow,
and also how to find the architecture file in either mode, write rules and
prove they can fail, read the results and point a person at the UI.
`init --install-skill` copies it into a project; to use it from any
directory, link it into the user's skills instead
(`ln -s "$PWD/skills/archgraph" ~/.claude/skills/archgraph`). A second
skill, [boring-code](skills/boring-code/SKILL.md), is about how the code
itself reads (one shape for every function, shallow nesting, comments that
say why) and is linked the same way; `init` does not install it.
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
GitNexus still ignores stylesheet imports, links routes only to literal
`fetch` calls and drops imports of packages. The browser smoke test
loads the embedded UI with mocked fetch and history. There is no production
Python component.

## Limitations and licensing

ArchGraph covers one repository per configuration and observes the GitNexus
relation kinds listed in `provider.edge_types`, plus stylesheets and CSS
classes with `provider.css: true`, client HTTP calls with
`provider.http: true` and imported packages with `provider.packages: true`. Symbol/function/AST exploration
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
