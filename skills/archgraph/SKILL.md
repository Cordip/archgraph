---
name: archgraph
description: Use ArchGraph to describe, check and show a project's architecture. Use it when changing module boundaries or dependencies, when writing or changing an architecture.yaml, when reading archgraph check/show/context/unused output, or when pointing a person at the ArchGraph UI; also for a project whose architecture.yaml lives outside it (archgraph --root <project> --config <file>).
---

# ArchGraph

ArchGraph compiles a desired architecture (`architecture.yaml`: nodes that
map files, and rules between them) against the file-level dependencies
GitNexus observes, checks the rules and serves a read-only UI. It does not
parse languages itself, except for stylesheets, HTTP calls and package
imports when the configuration turns those providers on.

## Find the architecture file

A project uses one of two modes:

- **In the project (the default).** `architecture.yaml` is at the
  project's root and committed with the code. Run commands from the project
  with no options: `archgraph check`.
- **Outside the project (development).** The file lives elsewhere, usually
  in the ArchGraph repository as `examples/<project>/architecture.yaml`,
  and is not committed to the project. Every command names both, with an
  absolute `--config` (a relative one is resolved against `--root`):

  ```bash
  ag() {
      archgraph --root /path/to/project \
          --config /path/to/archgraph/examples/project/architecture.yaml "$@"
  }
  ag check
  ```

  Use a shell function, not a variable holding the options: zsh does not
  split an unquoted variable, so `archgraph $OPTS check` fails and a
  following `grep -c` still prints a reassuring `0`.

When the project has no `architecture.yaml`, look for
`examples/*/architecture.yaml` in the ArchGraph repository whose header or
`project.name` names it, and ask the user before creating a new one.

In the development mode the project receives only generated, self-ignoring
directories: `.gitnexus/` (the GitNexus index) and `.archgraph/` (the
compiled IR and the provider cache). Nothing else may be written there: no
configuration, baseline, skill, Makefile target, CI job, hook or
documentation that mentions ArchGraph. Check that the project's
`git status` is unchanged after a run. Keep scratch copies of the
configuration in a temporary directory, not in the project.

## Before changing code

Run `archgraph context <relevant-node>`.
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

## Writing and changing the architecture

Do this only when the task is an architecture change. The README of the
ArchGraph repository ("Architecture source", "Actual versus desired
architecture") is the reference; `architecture.example.yaml` there is a
complete example.

- **Nodes** have dotted IDs (`app.domain`); every parent must exist. A node
  `maps` repository-relative globs (`**` crosses directories, `*` does not).
  A file belongs to the deepest matching node; matches in unrelated
  branches are ambiguous, unless one node has a higher `priority` (tests
  kept next to the code they test). `kind: external` nodes map no files,
  only `package:` globs.
- **Rules** check observed dependencies only: `deny_dependency`,
  `allow_only` (what a node may use), `allow_only_from` (who may use a
  node), `no_cycles` (among the children of `within`) and `layers` (upper to
  lower; a list entry is a layer of peers). Each rule names its
  `edge_types`, which must be among `provider.edge_types`.
- **`provider.edge_types`** decides what is observed. `IMPORTS` alone sees
  nothing in Rails-style code (constants are autoloaded): add `CALLS`,
  `EXTENDS`, `IMPLEMENTS`. `exclude_reasons` and `min_confidence` drop
  GitNexus's guesses (below).
- **`provider.css`, `provider.http`, `provider.packages`** add ArchGraph's
  own observations where GitNexus sees nothing: stylesheets and class names,
  frontend HTTP calls matched to backend routes (`FETCHES`), and imports of
  third-party packages (`package:<ecosystem>/<name>`).
- **`project.entry_points`** lists files loaded by name (a bundler config,
  `main.tsx`, test files, a server started from a string), so that
  `unused` does not report them. Add an entry only with the evidence (the
  script, Dockerfile or config that loads it) in a comment next to it.

Comment the YAML with the reason for each node and rule, and with the
evidence for every filter: the next reader cannot tell a deliberate
exclusion from a way to hide a violation.

## Proving a rule can fail

A clean check means something only if the rule could have failed. After
adding or changing a rule:

1. `archgraph show <parent>` and `archgraph context <node>`: the nodes the
   rule constrains must have observed files (`N file(s) (M observed)`), and
   the dependencies it allows must appear as observed edges. A rule over
   nodes with no observed dependencies is a rule over nothing.
2. Break it on purpose in a scratch copy of the configuration outside the
   project, for example with the layers reversed or a `deny_dependency`
   aimed at a dependency `show` lists, and run
   `archgraph --root <project> --config <copy> check <node>`. It must exit
   2 with violations of that rule. On lct-task3, reversing `core-layers`
   gives 19 violations.
3. Look at the coverage warnings. A rule node with few observed files is
   listed in `diagnostics.low_coverage`, and a passing check there is weak
   evidence.

A violation found this way in the real configuration is either work to do
in the code or evidence that the architecture is wrong. Say which, with the
file evidence, instead of weakening the rule.

## Reports

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

## Checking after edits

After source edits, from the repository root (in the development mode, with
`--root` and `--config` as above):

```bash
gitnexus analyze --index-only
archgraph check <relevant-node>
```

Always pass `--index-only`: without it GitNexus also writes its own agent
files into the project.

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

## Reading results

- A violation line reads `[rule] from -> to [KIND] × count`; `archgraph
  context <node>` and `check --json` give the file pairs behind it. A
  `no_cycles` violation also gives a layer order and suggested cuts: the
  fewest observed dependencies to remove, a starting point rather than a
  verdict.
- Warnings on stderr are part of the result. Coverage warnings (few
  observed files in a rule's node), unmapped or ambiguous files, provider
  anomalies and unindexed files all weaken a clean check; mention them when
  you report one.
- Some GitNexus relations are guesses. Before acting on a violation, read
  the evidence's `reason` and `confidence`: `global-name-fallback` (0.5)
  and `property-dispatch` (0.7) are the usual false positives. Excluding
  them (`exclude_reasons`, `min_confidence`) is an architecture change and
  needs evidence from the source in a comment, never "the check fails".
- GitNexus misses things: CoffeeScript, many Ruby constants, some
  TypeScript path aliases, Python `__init__.py` side effects. No observed
  edge is not proof of no dependency. The ArchGraph repository's
  `docs/gitnexus-limitations.md` lists the known gaps.
- "index changed while compiling" means GitNexus reindexed during the run
  (an auto-index service may be watching the project). Rerun it; it is not
  a result.

## The UI for people

`archgraph serve` (in the development mode with `--root` and `--config`)
serves a read-only UI on 127.0.0.1:7331; pass `--port` when that port is
taken, and never stop a server you did not start. It reloads by itself
after a reindex or a configuration edit and stays on the level it shows.
Point a person at a level with a deep link, `http://127.0.0.1:7331/?focus=<node id>`.

On a level, the canvas draws the entries (child nodes or files) in layers
from upper to lower with the dependencies as wires; entries in a violation
have a red cloud. **Table** lists the entries file by file and **Matrix**
is a dependency structure matrix. The node list on the left has every
violation; clicking one opens its level. Selecting a card, wire or violation
shows its evidence on the right, and a file or evidence line opens its
source below the details. Tell the person which level to open and what to
select rather than describing the drawing.

## Installing this skill

`archgraph init --install-skill` copies this file into a project's
`.claude/skills/archgraph/` or `.agents/skills/archgraph/`: the mode where
the architecture is committed with the project. For the development mode,
or to have the skill in every directory, link it into the user's skills
from the ArchGraph repository, so that it follows that repository:

```bash
ln -s /path/to/archgraph/skills/archgraph ~/.claude/skills/archgraph
```
