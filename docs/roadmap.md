# Roadmap: an architecture that is easy to edit

Goal: in a target project (the first one is lct-task3), a person or an agent
can change the desired architecture and the code towards it without the
description drifting from the code. Steps in the agreed order; the UI ideas
they refer to are described in [ui-backlog.md](ui-backlog.md).

## 1. One architecture file per project, in one of two modes

Each target project has one `architecture.yaml` (README, "Where the
architecture file lives"):

- **In the project (the default):** at the project's root, committed with
  the code.
- **Outside the project (development):** kept in this repository when the
  file should not be committed to the project, and every command passes
  `--root <project> --config <file here>`. lct-task3 uses this mode with
  `examples/lct-task3/architecture.yaml`; `archgraph serve` runs with it, so
  an edit shows in the UI at once.

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

From [ui-backlog.md](ui-backlog.md), in this order:

- [x] 5.1 The code viewer in the split right panel (a violation or evidence
  line opens the source): done, with the read-only `GET /api/source`
  limited to mapped files (see "Human focus UI" in the README).
- [x] 5.2 `archgraph snapshot` with a live graph diff: the command,
  `archgraph diff`, `GET /api/diff` (README, "Snapshots: what a
  refactoring changed") and the UI's **Compare** control, which draws the
  live graph against a snapshot (README, "Human focus UI").
- [ ] 5.3 Agent plan files drawn over the graph.

## 6. A skill for boring, readable code

An agent skill, next to [skills/archgraph](../skills/archgraph/SKILL.md),
that makes code written or refactored by an agent read as if one careful
person wrote all of it, in the spirit of early Go: one obvious way to do each
thing, so no file differs in style from any other. The architecture says
where code belongs; this skill says how it reads. Requirements from the
user:

- **Visible hierarchy.** A reader sees how the author thought: which
  function serves which purpose, from the entry point down to the helpers.
  In Rust the hierarchy may follow the data (types first, then what works on
  them) rather than the calls.
- **One shape for every function.** First set up everything it needs, then
  do the work, then hand the result on or return it. Errors are handled the
  same way everywhere.
- **Whitespace.** Blank lines separate the setup, work and return steps;
  indentation is never saved on.
- **`if` is always multi-line**, never a one-line conditional body.
- **Comments answer "why".** The code answers what and how; if it cannot, the
  code is what needs fixing.
- **Nesting no deeper than three or four levels.** Early returns and small
  functions instead.
- **Boring and standard.** Easy to read beats clever.
- **Optimised code is explained in detail.** Where performance matters, a
  comment describes how the algorithm works and why it is written this way,
  as the Go runtime does.

- Done: [skills/boring-code](../skills/boring-code/SKILL.md), one set of
  rules with examples in Rust, TypeScript and Python, and a review
  checklist. Formatters and a project's own written rules take precedence,
  and untouched code is restyled only as a separate change. It is linked
  into `~/.claude/skills/boring-code` on the development machine.
- Next, if the rules prove useful: a check for what a tool can measure
  (nesting depth, one-line `if`, function length), reported like
  `archgraph unused`, as candidates rather than failures.

## 7. A skill for using ArchGraph

The bundled [skills/archgraph](../skills/archgraph/SKILL.md) covers the
refactoring loop inside a repository that carries its own
`architecture.yaml`. It does not cover the way ArchGraph is used now: as a
local tool whose configuration for a target project lives outside that
project (for lct-task3, `examples/lct-task3/architecture.yaml` here), with the
work on the architecture done from this repository. The skill should cover:

- The two modes of step 1, and running every command against an outside
  target in the development mode: `--root <target>
  --config <file>`, where the index lives (`.gitnexus/`, `.archgraph/` in the
  target, ignored there) and that nothing else may be written into the
  target.
- Writing and changing the architecture: nodes and `maps`, the rule kinds,
  `entry_points`, the providers (`css`, `http`, `packages`), and proving that
  a new rule can fail (the edges it constrains are observed) rather than
  trusting a clean check.
- Reading results: `check` exit codes, `show`, `context`, `styles`, `http`,
  `packages`, `unused`, and false positives from GitNexus guesses
  (`exclude_reasons`, `min_confidence`) with evidence from the source.
- The UI for people: canvas, table and matrix, focus mode, trunks, colour
  modes, the code viewer, and how to point a person at a level or a
  violation.
- Installing it globally (`~/.claude/skills/archgraph`) so it works from
  any directory, as well as with `archgraph init --install-skill`.

- Done: the bundled skill now covers all of this, and it is linked into
  `~/.claude/skills/archgraph` on the development machine.
