---
name: archgraph
description: Use ArchGraph context, GitNexus exploration and architecture checks when changing module boundaries or dependencies in a repository containing architecture.yaml.
---

# ArchGraph workflow

Before architecture-sensitive changes, run `archgraph context <relevant-node>`.
Use `archgraph show` or the local UI to discover a relevant dotted architecture ID.
The context includes purpose, children/files, interfaces, observed dependencies,
constraints, violations, and concrete file evidence. `--json` is available for
structured consumption; `--evidence-limit 50` requests more examples.

Treat `architecture.yaml` as the desired architecture and the source of truth.
Do not modify it unless the user's task explicitly asks for an architecture
change. Never weaken a rule merely to make a check pass. Do not edit the compiled
`.archgraph/architecture.ir.json` file.

Use GitNexus `context`, `impact`, or `query` for deeper symbol-level investigation.
ArchGraph deliberately does not parse source languages or provide an AST view.
Inspect relevant source and preserve behavior while editing code.

When `provider.css: true` is set, `archgraph styles <node>` lists CSS classes
that are used but undefined, defined but unused, and shared between nodes.
Class expressions it could not resolve may use any class: check them before
removing a class reported unused.

When `provider.http: true` is set, `archgraph http <node>` lists the backend
routes with the frontend calls reaching them, and calls that reach no route.
A frontend's dependency on the backend appears as `FETCHES` edges.

When `provider.packages: true` is set, `archgraph packages [name]` lists
the third-party packages the code imports, with every importing file, line
and node. Packages appear as `package:<ecosystem>/<name>` (for example
`package:python/ortools`); unmapped ones belong to the `packages` node.

`archgraph unused [node]` lists files no observed code depends on. They are
candidates, not dead code: a tool, a runtime or a test runner may load them
by name, and GitNexus misses autoloading, script tags and dynamic imports.
Read the file and search for its name before removing anything. Files that
are loaded by name belong in `project.entry_points`, which is an
architecture change: edit it only when the task allows that.

After source edits, from the repository root:

```bash
gitnexus analyze --index-only
archgraph check <relevant-node>
```

`archgraph check <relevant-node> --reindex` combines these operations and respects
`GITNEXUS_BIN` / `provider.command`. It is incremental. After editing
`package.json`, `tsconfig*.json` or workspace files, use `--reindex=full`:
the incremental index keeps the old import resolution.

Exit codes are part of the verification contract:

- **0:** compilation succeeded and no matching observed violations were found.
- **1:** operational/config/provider/compiler failure. Resolve it and rerun; this
  is not a successful verification.
- **2:** compilation succeeded and matching architecture violations remain.
  Continue the refactoring loop. Do not declare completion while check exits 2.

If the repository has a baseline file (`architecture.baseline.json` next to
`architecture.yaml`), `check` fails only on observations that are not in it and
lists exactly those under "New since baseline". Fix them. Never run
`archgraph baseline` or edit the baseline file unless the user explicitly asks:
it accepts the current violations, just like weakening a rule.

Before declaring the task complete, run a repository-wide `archgraph check` when
changes could affect other modules, plus the project's behavioral tests. Report
any checks that could not run. Static evidence may be incomplete: no observed
edge is not proof of no runtime dependency. Manual edges and interface metadata
are descriptive, not verified runtime facts.

The web UI is read-only: `archgraph serve` defaults to 127.0.0.1:7331 and
reloads by itself after a reindex or an `architecture.yaml` edit.
