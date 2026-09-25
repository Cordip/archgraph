# Validation on lct-task3

lct-task3 is a field-service planner: a Python backend (FastAPI, OR-Tools)
and a React + TypeScript frontend styled by one global stylesheet. It is the
target ArchGraph 0.2 is built for: Python, TypeScript and CSS in one
repository. The frontend talks to the backend only over HTTP, so the two
halves share no code edges.

| Item | Value |
| --- | --- |
| lct-task3 commit | `8e8041b` (working tree, 87 Python and TypeScript files) |
| GitNexus | 1.6.12, `gitnexus analyze --index-only` |
| ArchGraph | release build, [architecture.yaml](architecture.yaml) with `provider.css: true` |

```bash
cd lct-task3 && gitnexus analyze --index-only
archgraph --root . --config /path/to/archgraph/examples/lct-task3/architecture.yaml check
archgraph --root . --config /path/to/archgraph/examples/lct-task3/architecture.yaml styles
```

The configuration lives here, not in lct-task3. Running it writes only the
`.archgraph/` working directory into the target.

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

## Excluded guesses

Two GitNexus relation reasons are excluded. Each produced edges that the
source code contradicts:

- `callable-value-flow` (0.8): `core/replan.py` → `api/store.py`. `replan.py`
  imports nothing from `planner.api`.
- `property-dispatch` (0.7): `api.ts` → `usePlanningRequests.ts` and
  `components/MapView.tsx` → `useSimulation.ts`. Neither file imports the
  other. This is the same false positive as in zammad.

## Findings

`check` reports three violations (the first finding appears once as
`IMPORTS` and once as `CALLS`). Both findings are real:

1. **The core depends on data preparation.** `core/validate.py` imports
   `ingest/equipment.py` at module level; `explain.py` and `reasons.py` import
   it lazily; `core/control.py` imports `ingest/beeline.py` lazily. The
   architecture document calls data preparation offline, but equipment
   demand and stock are computed by `ingest/equipment.py` for every
   validation. Moving `equipment.py` into the core (and the control-file
   reader out of it) would make the layers hold.
2. **A component imports types from an app hook.**
   `components/Simulation.tsx` imports `SimEvent`, `Simulation` and `Speed`
   from `useSimulation.ts`. It is type-only, but it ties the component to the
   hook's module.

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

## Coverage

83 of the 89 mapped files are observed. The other six are the five empty
`__init__.py` package markers and `vite.config.ts`, which nothing imports.
The Leaflet and `@fontsource` stylesheets are read from
`frontend/node_modules`, so classes that only override Leaflet's
(`.leaflet-tooltip`) count as used.
