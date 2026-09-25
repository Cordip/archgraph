# Validation on lct-task3

lct-task3 is a field-service planner: a Python backend (FastAPI, OR-Tools)
and a React + TypeScript frontend styled by one global stylesheet. It is the
target ArchGraph 0.2 is built for: Python, TypeScript and CSS in one
repository. The frontend talks to the backend only over HTTP, so the two
halves share no code edges, only `FETCHES` edges from HTTP calls.

| Item | Value |
| --- | --- |
| lct-task3 commit | `8e8041b` for the findings (87 Python and TypeScript files); fixed on branch `refactor/architecture-boundaries` of the fork `Cordip/lct-task3` |
| GitNexus | 1.6.12, `gitnexus analyze --index-only` |
| ArchGraph | release build, [architecture.yaml](architecture.yaml) with `provider.css: true` and `provider.http: true` |

```bash
cd lct-task3 && gitnexus analyze --index-only
archgraph --root . --config /path/to/archgraph/examples/lct-task3/architecture.yaml check
archgraph --root . --config /path/to/archgraph/examples/lct-task3/architecture.yaml styles
archgraph --root . --config /path/to/archgraph/examples/lct-task3/architecture.yaml http
```

Since branch `refactor/architecture-boundaries`, lct-task3 carries the same
configuration as its own `architecture.yaml`, with comments and titles in
Russian (the repository's language), and runs it with `make arch`. The copy
here is the one this validation used and ArchGraph's tests parse. Running
ArchGraph writes only `.archgraph/` into the target, which ignores itself.

## Nodes and rules

The backend layers follow lct-task3's `docs/architecture.md`: data
preparation (`ingest`) builds core models from the raw export, the core
(`solver`, `validate`, `replan`, ...) computes plans, and the web service and
the CLI sit on top. `paths.py` only names data directories and sits below
everything.

The frontend is split by role: app state and hooks, components, pure view
helpers, the HTTP client, the API types and the stylesheet. Classes a
component uses become `USES_CLASS` edges from the component to
`styles.css`, so the stylesheet is the lowest frontend layer.

The frontend reaches the backend through `api.ts` only. Three
`deny_dependency` rules over `FETCHES` keep it that way: app state,
components and view helpers must not call the backend themselves. A
temporary `fetch('/api/reference')` in a component fails the check with the
component and `api/app.py` as evidence.

## Excluded guesses

Two GitNexus relation reasons are excluded. Each produced edges that the
source code contradicts:

- `callable-value-flow` (0.8): `core/replan.py` → `api/store.py`. `replan.py`
  imports nothing from `planner.api`.
- `property-dispatch` (0.7): `api.ts` → `usePlanningRequests.ts` and
  `components/MapView.tsx` → `useSimulation.ts`. Neither file imports the
  other. This is the same false positive as in zammad.

## Findings

At `8e8041b`, `check` reports three violations (the first finding appears
once as `IMPORTS` and once as `CALLS`). Both findings are real:

1. **The core depends on data preparation.** `core/validate.py` imports
   `ingest/equipment.py` at module level; `explain.py` and `reasons.py` import
   it lazily; `core/control.py` imports `ingest/beeline.py` lazily. The
   architecture document calls data preparation offline, but equipment
   demand and stock are computed by `ingest/equipment.py` for every
   validation.
2. **A component imports types from an app hook.**
   `components/Simulation.tsx` imports `SimEvent`, `Simulation` and `Speed`
   from `useSimulation.ts`. It is type-only, but `Simulation` was
   `ReturnType<typeof useSimulation>`, which ties the component to the hook's
   implementation.

`styles` reports:

- **12 undefined classes**, used in markup but defined in no stylesheet:
  `compact`, `compare-table`, `eng-list`, `engineers`, `ev-list`,
  `events-panel`, `g-n`, `job`, `params`, `q-list`, `speed`, `stop-id`.
  Checked by hand: none occurs in `styles.css`. Most are hooks for layout
  that never got a rule; each either needs a rule or can go.
- **2 unused classes.** `.cand-detail` is used nowhere. `.failed` is a false
  report: `Simulation.tsx` writes `` `ev ${item.status}` `` and `status` can be
  `'failed'`. The report lists `item.status` as an unresolved expression,
  and says unused classes may come from such expressions.
- **19 classes possibly used dynamically**, e.g. `em-*` from
  `` `em-${status}` `` in `mapIcons.ts` and `s-*` from status classes.
- **12 classes shared between the app node and components**, e.g. `.kpi`,
  `.spinner`, `.empty`. They are the de facto shared UI vocabulary.

`http` matches 16 of the 19 routes to calls in `api.ts`. GitNexus alone
links none of them (docs/gitnexus-limitations.md, section 11). The three
uncalled routes are `/api/health`, used by the container health check, and
`/` and `/{path:path}`, which serve the built frontend. No call reaches a
missing route or uses a wrong method.

### Fixes

Branch `refactor/architecture-boundaries` fixes every finding without
changing behavior. The target's backend tests pass before and after (440
passed, 3 skipped), and the frontend builds (`tsc -b && vite build`) after:

1. `equipment.py` moved into `core/` (it depends only on core models and
   paths), and `control_brigades` moved from `ingest/beeline.py` into
   `core/control.py`, next to the attribute it reads.
2. `SimEvent`, `Speed` and an explicit `Simulation` interface live in
   `sim.ts`; `useSimulation` declares it as its return type, so `tsc` checks
   the contract.
3. The 12 classes without a rule were removed from the markup (none ever had
   a rule in the history, and nothing selects them), and so was the unused
   `.cand-detail` rule.

After the fixes, `check` reports no violations and `styles` no undefined
classes; `.failed` remains as the known false report.

## Coverage

83 of the 89 mapped files are observed. The other six are the five empty
`__init__.py` package markers and `vite.config.ts`, which nothing imports.
The Leaflet and `@fontsource` stylesheets are read from
`frontend/node_modules`, so classes that only override Leaflet's
(`.leaflet-tooltip`) count as used.
