# Agent guide

Rules for coding agents working in this repository. Read this file before
changing anything, and add to **Gotchas** whenever something surprises you.

## Project in one paragraph

ArchGraph is a single Rust crate (`archgraph` binary). It compiles a desired
architecture (`architecture.yaml`) against file-level dependencies observed by
the external GitNexus CLI, checks rules and serves a read-only UI. Language
parsing belongs to GitNexus. The one exception is CSS, which GitNexus cannot
parse: with `provider.css`, `src/provider/css/` reads stylesheets
(lightningcss) and class names in scripts (tree-sitter). Do not extend it to
anything GitNexus parses. See
[README.md](README.md) for behavior, [DESIGN.md](DESIGN.md) for the original
design and [docs/](docs/) for provider limitations.

## Workflow

- Before declaring a change done, run:
  ```bash
  cargo fmt --all -- --check
  cargo clippy --all-targets     # must print no warnings
  cargo test --all-targets
  ```
- For any provider, compiler, rule or projection change, also run against a
  real GitNexus index. The reference target is zammad
  ([examples/zammad](examples/zammad/)):
  ```bash
  cargo build --release
  ./target/release/archgraph --root /path/to/zammad \
      --config "$PWD/examples/zammad/architecture.yaml" check
  ```
  Tests use a fake provider (`tests/fixtures/fake_gitnexus.py`); passing tests
  alone never proved GitNexus compatibility (see Gotchas). For provider
  changes, and after upgrading GitNexus, also run the contract test against
  the real CLI (TypeScript and Ruby fixtures, about 20 s, isolated `HOME`):
  ```bash
  cargo test --test gitnexus_contract -- --ignored
  ```
- UI changes: `node --check src/web/app.js`, then the browser smoke test
  (`uv run --with playwright python tests/web_smoke.py`, with
  `ARCHGRAPH_CHROMIUM` pointing at a Chromium binary). Also look at the real UI
  with `archgraph serve`.
- Every bug fix gets a test that fails on the old code. Check that it does.
- One logical change per commit. Commit messages explain why.

## Code rules

- Match the surrounding style: compact, `anyhow` errors with actionable
  messages, deterministic ordering (`BTreeMap`/`BTreeSet`, explicit sorts).
- Output must be deterministic: same inputs produce byte-identical IR and text.
- Fail closed. A provider or compile failure must never produce an empty or
  "clean" result. Anything ArchGraph cannot map goes to diagnostics, never
  silently dropped or guessed.
- A clean check must mean something. If a configuration makes a rule unable to
  fail, reject it at validation time.
- Everything provider-specific lives in `src/provider/`. The compiler, rules
  and projection only see `CodeEdge`s.
- No Python or Node in production code. Python exists only as a test fixture and
  the optional UI smoke test.
- Strings that reach Cypher queries must be validated identifiers
  (`config::valid_edge_type`), never user text.
- The UI stays dependency-free plain JavaScript. Build DOM with `textContent`
  and `createElementNS`, never `innerHTML`.
- Documentation is in English.

## Do not

- Do not modify, open issues against or send PRs to GitNexus. Record provider
  problems in [docs/gitnexus-limitations.md](docs/gitnexus-limitations.md).
- Do not edit the generated `.archgraph/architecture.ir.json`.
- Do not weaken a rule or filter in `examples/zammad/architecture.yaml` just to
  make a check pass. Change it only with evidence from the source code, and
  record the evidence in `examples/zammad/README.md`.

## Gotchas

Add new entries at the end: what happened, why, and what to do.

1. **GitNexus truncates piped stdout at 64 KiB.** It is a Node CLI that exits
   before draining a pipe. Capture its output in a file (see
   `GitNexusCliProvider::query`). A plain Python fake does not reproduce this;
   the `node_like_large` fixture mode does (non-blocking write, then exit).
2. **`tokio::process::Command::output()` replaces configured stdio with pipes.**
   To redirect a child's stdout to a file, use `spawn()` and
   `wait_with_output()`.
3. **GitNexus `IMPORTS` include Markdown links** (`reason: markdown-link`).
   They can be most of all `IMPORTS`. They are excluded by default.
4. **Rails code has almost no `IMPORTS`.** Constants are autoloaded, so Ruby
   dependencies appear only as `CALLS`, `EXTENDS` and `IMPLEMENTS`. An
   IMPORTS-only config reports a Rails app as clean.
5. **Some GitNexus relations are guesses.** Check `reason` and `confidence`
   before trusting a violation. `global-name-fallback` (0.5) and
   `property-dispatch` (0.7) produced every false positive in zammad.
6. **Ruby open classes merge.** A file that reopens `class Foo::Bar` receives
   the relations of the real `Foo::Bar` definition, such as its `include`s.
7. **Coverage is uneven.** GitNexus does not parse CoffeeScript and misses many
   Ruby constants and TypeScript path aliases. Check `observed_file_count` and
   the coverage warnings before calling any result clean.
8. **`pkill -f <pattern>` in an agent shell can kill the shell itself**, since
   the shell's own command line contains the pattern. Run servers as tracked
   background tasks and stop them by task ID.
9. **In zsh, `grep --include=*.rb` fails with "no matches found".** Quote the
   glob: `--include='*.rb'`.
10. **Incremental `gitnexus analyze` ignores resolver configuration changes.**
    Its parse cache replays import resolution for unchanged source files, so
    edits to `package.json`, `tsconfig*.json` or workspace files have no effect.
    Use `archgraph compile --reindex=full` (it runs
    `gitnexus analyze --index-only --force --no-parse-cache`).
11. **GitNexus resolves `#` imports only via a named, workspace-admitted
    package.** It never falls back to `tsconfig` `paths` for them. Read the
    GitNexus `dist/` source to find causes like this; do not guess from the
    symptoms. Details in `docs/gitnexus-limitations.md`.
12. **zsh does not word-split unquoted variables.** `A="--root x"; cmd $A`
    passes one argument, the command fails, and a following `grep -c` still
    prints a reassuring `0`. Use a shell function or an array, and check that
    the command produced real output before trusting a count.
13. **A stale `archgraph serve` keeps the port.** A new server on the same port
    exits with a bind error while the browser still shows the old build. Stop
    earlier servers before re-checking UI changes.
14. **An auto-index service can rewrite the GitNexus index mid-run.** This
    machine runs `gitnexus-auto-index.service`, which reanalyzes every
    registered repository on file changes, including ArchGraph's own
    `.archgraph/` output. Two identical runs then disagree. ArchGraph now fails
    with "index changed while compiling"; rerun after indexing ends. Check
    `journalctl --user -u gitnexus-auto-index` before blaming ArchGraph for
    nondeterminism. Registering a test clone (`gitnexus analyze`) puts it
    under the service's watch.
15. **`cargo test` does not rebuild `target/release`.** Real-repository checks
    use the release binary; run `cargo build --release` after every change, or
    an old binary rejects new config fields or shows old behavior.
16. **Provider results are cached per index fingerprint.** A test whose fake
    provider changes its answers (a different `ARCHGRAPH_FAKE_MODE`) while
    `.gitnexus/meta.json` stays the same gets the previous run's cached
    result. Pass `--no-cache`, or change `meta.json` as a real reindex would.
17. **Reproduce a provider failure before explaining it.** A zammad compile
    failed with "row 2056 has empty/null source or target" while the
    auto-index service happened to be running, and was first blamed on a
    torn read. The same row failed again with the service idle: GitNexus had
    stored symbols without a file path (docs, section 9). Rerun the failing
    query directly with `gitnexus cypher` when indexing is idle, and look at
    the offending rows, before writing down a cause.
18. **Windows checkouts turned test sources into CRLF.** GitHub's Windows
    runner checks out with `core.autocrlf=true`, so a YAML string embedded in a
    test source got `\r\n` and `CONFIG.replace("...\n...")` silently matched
    nothing: a local Windows run from a `git archive` copy passed, CI failed.
    `.gitattributes` now forces LF. Test Windows from a real checkout.
19. **An "unused" CSS class may still be used.** A class computed from data
    (`` `ev ${item.status}` `` in lct-task3) cannot be resolved statically:
    `.failed` looked unused, but `status` can be `'failed'`. `archgraph
    styles` lists such expressions as unresolved. Read them before calling a
    class dead, and never delete CSS on the report alone.
20. **`min_confidence` also filters ArchGraph's own CSS edges.** A class
    defined in two stylesheets gives `classname-ambiguous` edges at 0.5, which
    `min_confidence: 0.6` drops (they are counted in
    `stats.filtered_edge_count`).
