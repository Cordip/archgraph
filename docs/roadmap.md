# Roadmap: an architecture that is easy to edit

Goal: in a target project (the first one is lct-task3), a person or an agent
can change the desired architecture and the code towards it without the
description drifting from the code. Steps in the agreed order; the UI ideas
they refer to are described in [ui-backlog.md](ui-backlog.md).

## 1. One architecture file per project

Each target project has one description, kept where the work on it happens.
For lct-task3 that is `examples/lct-task3/architecture.yaml` in this
repository: ArchGraph is a local tool there and stays out of lct-task3's
repository (no `architecture.yaml`, no `make arch`). `archgraph serve` runs
with this file, so an edit shows in the UI at once.

- Done: `ui-redesign` merged into `main` and the binary reinstalled.

## 2. Finer nodes and rules

A clean check must mean something. A node that maps a whole directory with
no layers inside it cannot fail however its files depend on each other.

- Done for lct-task3: `core-layers` (planning; explanations and metrics;
  validator; domain rules; models and utilities), `api-layers` (endpoints
  over jobs, previews and the store) and `component-layers` (screens, panels,
  map, shared elements). Every observed dependency between them points down,
  so the check stays clean; a violation they find later is either a
  refactoring target or evidence that a layer is wrong.

## 3. When the check runs

Decided for lct-task3: by hand only (`archgraph check` with the example
configuration), with no CI job and no git hook, since ArchGraph is a personal
tool there. Step 2 found no violations, so there is no baseline either. Revisit if a CI job or a pre-push hook
(`core.hooksPath`, failing when `archgraph` or `gitnexus` is missing) becomes
worth it; the README's "Checking architecture in your CI" describes the job.

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
