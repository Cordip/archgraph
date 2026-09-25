# GitNexus limitations that affect ArchGraph

ArchGraph sees only what GitNexus reports. This page lists known gaps,
how to detect them and what to do about them. Every entry is backed by the
zammad validation run ([examples/zammad](../examples/zammad/README.md)),
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

## ArchGraph-side limitations

None known at the moment. Co-located tests used to be one: they can now be
assigned to a test node with a node `priority` (see the README).
