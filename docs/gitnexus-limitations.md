# GitNexus limitations that affect ArchGraph

ArchGraph sees only what GitNexus reports. This page lists known gaps,
how to detect them and what to do about them. Every entry is backed by the
zammad validation run ([examples/zammad](../examples/zammad/README.md)) or,
for CSS, HTTP calls, packages and `__init__.py`, by lct-task3
([examples/lct-task3](../examples/lct-task3/README.md)),
GitNexus 1.6.12. Record new findings here; see [AGENTS.md](../AGENTS.md).

Detect gaps with `observed_file_count` (shown by `show`, `context` and the UI)
and with the coverage warnings `compile` and `check` print. ArchGraph also
lists every mapped file missing from the index altogether
(`diagnostics.unindexed_files`, counted per node and in the coverage warnings):
such a file was never read, which is different from a file read and found to
have no dependencies. A node where most
files have no observed dependency cannot fail a rule in any meaningful way.

## 1. `#` subpath imports resolve only through a named workspace package

**Symptom.** In zammad about 10,000 TypeScript/Vue imports use `#shared/...`,
`#desktop/...`, `#mobile/...` or `#tests/...` and none of them became `IMPORTS`.
Only about 1,900 relative imports did. Frontend nodes showed 44–52% coverage.

**Cause** (read in `dist/core/ingestion/languages/typescript/module-resolution.js`).
GitNexus resolves any specifier starting with `#` only through the `imports`
field of the importing file's owning package (`resolveSubpathImport`). It never
falls back to `tsconfig` `paths`, although `tsc` checks `paths` for such
specifiers as well. A `package.json` counts as an owning package only if

- it has a `name` (`readManifest` skips unnamed manifests), and
- its directory is admitted by the workspace declaration (`pnpm-workspace.yaml`
  `packages`, `package.json` `workspaces`, or `lerna.json`), unless it is the
  repository root.

zammad's `app/frontend/package.json` declares the `imports` but has no `name`,
and its `pnpm-workspace.yaml` has no `packages` list, so both conditions fail.
Its `tsconfig.base.json` does declare the same aliases under `paths`; GitNexus
follows `extends` correctly, but never consults `paths` for `#` specifiers.

**Confirmed workaround** (in a local checkout only; it changes the analyzed
repository, so do not commit it there):

```bash
printf 'packages:\n  - app/frontend\n' >> pnpm-workspace.yaml
# add "name": "zammad-frontend" to app/frontend/package.json
gitnexus analyze --index-only --force --no-parse-cache .
```

Result on zammad: resolved TypeScript/Vue `IMPORTS` went from about 1,900 to
8,627. Frontend coverage went to 89–97% (with co-located test helpers
excluded, see [limitations of ArchGraph](#archgraph-side-limitations)). It also
exposed one real violation that was invisible before:
`shared/composables/useStickyHeader.ts` imports a type from
`#mobile/components/layout/LayoutHeader.vue`.

## 2. Incremental `analyze` does not pick up resolver configuration changes

**Symptom.** After the manifest edits above, `gitnexus analyze --index-only`
finished in 30 s and reported the same imports as before.

**Cause.** The parse cache replays earlier per-file results, including import
resolution, when source files are unchanged. Changes to `package.json`,
`tsconfig*.json` or workspace files do not invalidate it.

**What to do.** After changing any resolver configuration, run
`archgraph compile --reindex=full` (or `gitnexus analyze --index-only --force
--no-parse-cache`). Plain `--reindex` runs the incremental `analyze` and does
not help here.

## 3. Rails autoloading: Ruby dependencies are mostly not `IMPORTS`

**Symptom.** `app/models/ticket.rb`, zammad's central model, has no `IMPORTS`
edge. Only 69 of 2,819 Ruby files under `app/` and `lib/` use `require`.

**Cause.** Zeitwerk autoloads constants; there is nothing to import.

**What to do.** Observe `CALLS`, `EXTENDS` and `IMPLEMENTS` as well
(`provider.edge_types`). Even then, coverage stays low: GitNexus logged 10,747
Ruby call sites it refused to resolve. In zammad 96% of `app/graphql`, 94% of
`app/policies` and 66% of `app/models` files have no observed dependency. No
ArchGraph-side fix is known.

## 4. Guessed relations

GitNexus marks inferred relations with `reason` and `confidence`:

| reason | confidence | Seen in zammad |
| --- | --- | --- |
| `global-name-fallback` | 0.5 | `app/models/ticket.rb -> spec/requests/...` |
| `property-dispatch` | 0.7 | `el.focus()` on a DOM element linked to a Vue component's `focus` in another app |
| `markdown-link` (`IMPORTS`) | 0.8 | documentation links; 70% of all `IMPORTS` in another repository |

All frontend violations in the first zammad run were `property-dispatch`
guesses. Use `provider.min_confidence` and `provider.exclude_reasons`
(`markdown-link` is excluded by default).

## 5. Ruby open classes are merged

A file that reopens a class (`class KnowledgeBase; class Category` in
`lib/knowledge_base/category/permission.rb`) receives relations of the real
definition, such as the model's `include`s. The coupling is real, but the
file-level evidence names the wrong file. No workaround.

## 6. CoffeeScript is not parsed

zammad's legacy desktop UI (671 `.coffee` files in `app/assets`) is 99%
unobserved. The files are in the index (only 6 of 814 files there are not), but
GitNexus extracts no relations from them. No workaround in ArchGraph; treat
such nodes as unchecked.

## 7. The index can change while ArchGraph reads it

ArchGraph pages through query results. If `gitnexus analyze` rewrites the index
in between, for example an auto-index service reacting to file changes, the
pages mix two graphs. On the validation machine this made two identical runs
disagree. ArchGraph compares the size and modification time of the index
(`meta.json`, `lbug`) before and after querying and fails with "index changed
while compiling" instead of reporting the mix. When a query fails outright and
the index changed meanwhile, the same error is reported, with the query
failure as its cause, instead of blaming GitNexus compatibility. This is a
precaution; it has not been observed. (An empty-path failure on zammad first
looked like such a torn read, but was the defect in section 9.) Rerun when indexing has
finished. Writing `.archgraph/` inside the repository is itself a file change
such a service may react to.

Every `analyze` rewrites `meta.json` and `lbug`, even one that reports "index
current" and changes nothing. ArchGraph's provider cache is keyed on the same
fingerprint, so a periodic reindex sweep invalidates it: results are reused
only between sweeps. That is conservative; a content hash of the 500 MB
database would cost more than the queries it saves.

## 8. Some directory names are never indexed

GitNexus skips a hard-coded list of directory names at any depth
(`dist/config/ignore-service.js`), including `__tests__`, `__mocks__`,
`coverage`, `logs`, `log`, `tmp`, `temp` and `cache`. In zammad none of the 810
files under `app/frontend/**/__tests__/` are in the index, so they look like
files without dependencies. A source directory that happens to be called
`cache` or `log` disappears the same way. To index such a directory, add a
negation to `.gitnexusignore` in the analyzed repository, e.g. `!__tests__/`,
and reindex with `--force`.

## 9. Incremental indexing can leave symbols without a file path

**Symptom.** After a few incremental `analyze` runs on zammad in which files
had changed (a file moved and moved back), dependency queries returned rows
with an empty target, and ArchGraph failed with "row 2056 has empty/null
source or target".

**Cause.** The index then held nodes whose `filePath` is the empty string
(not null), although their ID still names the file, e.g.
`Const:app/frontend/.../KnowledgeBaseAnswerTopBarHeader.vue:router`. At worst
about 1,600 relations touched such nodes, five of them `CALLS` between
different files. `WHERE ... IS NOT NULL` does not exclude an empty string.
A forced rebuild (`--force --no-parse-cache`) left zero such nodes, and a
later incremental sweep that changed nothing kept it at zero; moving one file
and back with plain incremental `analyze` produced one again.

**What ArchGraph does.** Such an edge is reported as a provider anomaly
("provider returned an empty file path") with a warning pointing here, and
the rest of the graph compiles. The file is not guessed from the node ID.
Run `archgraph compile --reindex=full` to clear them.

## 10. CSS is not parsed, and stylesheet imports are dropped

**Symptom.** In lct-task3 (React + TypeScript, one global `styles.css`),
`main.tsx` has `import './styles.css'`, but GitNexus reports no relation to
the stylesheet at all. A frontend's dependency on its styles is invisible.

**Cause.** GitNexus 1.6.12 has no CSS grammar: `SupportedLanguages` has no
CSS entry, and `.css` appears only in the UI's syntax-highlighting map.
Stylesheets still become `File` nodes, so they are not reported as
unindexed. Import resolution only targets files that were parsed
(`allFilePaths` in `dist/core/ingestion/scope-resolution/pipeline/run.js`),
so the side-effect import of a stylesheet resolves to nothing and is
dropped. Upstream issue #1198 ("Support CSS files") is open; 1.6.13-rc.34
behaves the same. The contract test
`gitnexus_drops_stylesheet_imports_and_archgraph_supplies_them` fails once
this changes.

**What ArchGraph does.** `provider.css: true` makes ArchGraph read
stylesheets and class names itself and add `IMPORTS` and `USES_CLASS`
edges (README, "Stylesheets and CSS classes"). It never asks GitNexus for
`USES_CLASS`.

## 11. Client HTTP calls reach routes only as literal URLs

**Symptom.** In lct-task3, GitNexus reports all 19 FastAPI routes
(`HANDLES_ROUTE`, `Route` nodes with method and handler file), but not one
`FETCHES` relation from the frontend: the frontend's dependency on the
backend is invisible.

**Cause.** Client calls are captured by tree-sitter queries in
`dist/core/ingestion/tree-sitter-queries.js`: `fetch(...)` with a string or
template argument, and a string argument to wrappers named like `apiFetch`,
`fetchJSON` or `httpGet`. lct-task3 calls a wrapper named `request('/plans')`,
which no pattern names. Its direct calls use a base constant,
`` fetch(`${BASE}/scenarios/upload`) ``: `normalizeFetchURL`
(`dist/core/ingestion/route-extractors/nextjs.js`) turns that into
`[param]/scenarios/upload`, which does not start with `/`, and drops it. The
wrapper's own `fetch(BASE + path)` passes no string or template at all, so it
has no URL to match.
A literal `fetch('/api/items')` does get a `FETCHES` edge (reason
`fetch-url-match`); the contract test
`gitnexus_routes_and_archgraph_client_calls_meet` checks both behaviors.

**What ArchGraph does.** `provider.http: true` reads client calls itself,
evaluating base constants and same-file wrappers, and matches them to the
routes GitNexus reports (README, "HTTP calls between frontend and backend").
Where GitNexus links a call too, the two observations merge.

## 12. Imports of packages are dropped without a trace

**Symptom.** lct-task3's `backend/planner/core/solver.py` has
`from ortools.constraint_solver import pywrapcp, routing_enums_pb2`, and
`backend/tests/test_solver_progress.py` imports it too, but no GitNexus
query can say who uses OR-Tools: the index has no node for `ortools` and no
relation from either file to anything outside the repository. The same holds
for `react`, `leaflet`, `fastapi` and every other package.

**Cause.** GitNexus 1.6.12 turns an import into an edge only when it
resolves to a repository file. `makeEdgeDrafts`
(`dist/_shared/scope-resolution/finalize-algorithm.js`) marks an import
whose `resolveImportTarget` finds no file as `linkStatus: 'unresolved'`, and
`emitImportEdges`
(`dist/core/ingestion/scope-resolution/graph-bridge/imports-to-edges.js`)
skips every edge with `targetFile === null`. The only record is an
`unresolvedEdges` count that nothing reads. The resolvers say so
themselves: for TypeScript, "an external package ... resolves to nothing"
(`languages/typescript/module-resolution.js`); `node_modules` is on the
default ignore list, so a package is never a target. The schema has no label
or relation for a package (`dist/core/lbug/schema.js`; `Module` is used for
COBOL and Ruby only), and the import specifier is kept on in-memory objects
only: `File` stores `id`, `name`, `filePath` and `content`. The contract test
`gitnexus_drops_package_imports_and_archgraph_supplies_them` fails once this
changes.

A related guess: a single-segment Python import resolves by a repository-wide
suffix match (`resolveAbsoluteFromFiles` in
`languages/python/import-target.js`), so `import redis` becomes an `IMPORTS`
edge to any `…/redis.py` in the repository, even one that is not on the
import path (reproduced: `app/cache.py` with `import redis` gets an edge to
`tools/redis.py`).

**What ArchGraph does.** `provider.packages: true` makes ArchGraph read
import statements in Python and TypeScript/JavaScript itself and add
`IMPORTS` edges from the importing file to `package:<ecosystem>/<name>`
(README, "Imported packages"). It decides local or package from the
repository's own files and `package.json` files, not from GitNexus's Python
guess; an import that could be either is reported, not observed.

## 13. Importing a Python module gives its package's `__init__.py` no edge

**Symptom.** In lct-task3, `from planner.core import solver` appears in many
files, yet `backend/planner/core/__init__.py` has no incoming `IMPORTS` or
`CALLS`. A query for relations into any `__init__.py` returns only `CONTAINS`
from the folder. `archgraph unused` then lists every package's
`__init__.py` as a file with no observed users, although Python runs it
before any module of the package.

**Cause.** GitNexus 1.6.12 resolves each import to exactly one file
(`resolvePythonImportTarget` in `languages/python/import-target.js`). For
`from package import name` it takes the package's `__init__.py` only when
that file defines `name`, and otherwise the submodule `name.py`; `import
a.b.c` resolves to `a/b/c.py`. The parent packages whose `__init__.py`
Python executes on the way are not recorded.

**What to do.** Declare them as entry points (`"backend/**/__init__.py"` in
`project.entry_points`), as the lct-task3 example does. An `__init__.py`
that defines names other modules import still gets its edges.

## ArchGraph-side limitations

None known at the moment. Co-located tests used to be one: they can now be
assigned to a test node with a node `priority` (see the README).
