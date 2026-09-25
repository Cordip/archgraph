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

After source edits, from the repository root:

```bash
gitnexus analyze --index-only
archgraph check <relevant-node>
```

`archgraph check <relevant-node> --reindex` combines these operations and respects
`GITNEXUS_BIN` / `provider.command`. It never forces a full rebuild.

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

The web UI is read-only: `archgraph serve` compiles a snapshot and defaults to
127.0.0.1:7331. Restart it to refresh after reindexing and code edits.
