# Roadmap: an architecture that is easy to edit

Goal: in a target project (the first one is lct-task3), a person or an agent
can change the desired architecture and the code towards it without the
description drifting from the code. Steps in the agreed order; the UI ideas
they refer to are described in [ui-backlog.md](ui-backlog.md).

## 1. One architecture file per project

The project's own `architecture.yaml` is the only description. ArchGraph's
`examples/<project>/architecture.yaml` follows it, never the other way round,
and `archgraph serve` runs with the project's file, so an edit shows in the
UI and in the project's own check (`make arch` in lct-task3) at once.

- lct-task3: bring `entry_points`, `provider.packages`, `libs.ortools` and
  `only-the-core-uses-the-solver` from the example into the project's file.
- Merge `ui-redesign` into `main` and reinstall the binary
  (`cargo install --path .`), so that the project's `make arch` and
  `archgraph serve` use the current build.

## 2. Finer nodes and rules

A clean check must mean something. A node that maps a whole directory with
no layers inside it cannot fail however its files depend on each other.

- lct-task3: split `core` (17 files) into layers (models and helpers;
  travel, geometry and zones; solver and validation; replanning,
  explanations and metrics), `api` into routes, jobs, candidates and store,
  and the frontend components by screen area. Derive the layers from
  `docs/architecture.md` and the observed dependencies; a violation the new
  rules find is either a refactoring target or evidence that the layer is
  wrong, and is recorded as such.

## 3. Checks in CI

- `archgraph baseline` for the violations accepted in step 2, then a CI job
  that runs `archgraph check` and fails only on new ones (README, "Checking
  architecture in your CI").

## 4. "What if" edits in the UI

Moving a file to another node or drawing a denied dependency on the canvas
previews which violations appear and disappear, and produces the YAML patch
to copy. The server stays read-only and comments in the YAML file are never
rewritten; writing the file directly is a separate decision that needs a
comment-preserving editor.

## 5. Refactoring towards the architecture

From [ui-backlog.md](ui-backlog.md), in this order: the code viewer in the
split right panel (a violation or evidence line opens the source), then
`archgraph snapshot` with a live graph diff, then agent plan files drawn over
the graph.
