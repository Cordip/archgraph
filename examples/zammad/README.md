# Validation on zammad

[zammad](https://github.com/zammad/zammad) is a large, long-lived help desk:
a Rails backend, a legacy Spine/CoffeeScript desktop UI and a newer Vue/TypeScript
frontend. It was the first real repository ArchGraph ran against.

| Item | Value |
| --- | --- |
| zammad commit | `5b396c16c1d150bbcacf42e593a66d860527ae50` (2026-09-24), shallow clone |
| GitNexus | 1.6.12, `gitnexus analyze --index-only` (148 s, 65,830 nodes, 117,163 edges) |
| ArchGraph | Rust 1.98 release build, [architecture.yaml](architecture.yaml) |
| Compile time | about 16 s for 10,203 mapped files |

```bash
git clone --depth 1 https://github.com/zammad/zammad
cd zammad && gitnexus analyze --index-only
archgraph --root . --config /path/to/archgraph/examples/zammad/architecture.yaml check
archgraph --root . --config /path/to/archgraph/examples/zammad/architecture.yaml serve
```

## ArchGraph defects found and fixed

1. **Provider output truncated at 64 KiB.** GitNexus is a Node CLI that exits
   before draining a piped stdout. Every page larger than the pipe buffer
   arrived cut off, and `archgraph compile` failed on zammad immediately with
   `stdout is not JSON`. Output is now captured in a temporary file.
2. **Rules over unobserved relation kinds always passed.** Only `IMPORTS` was
   fetched, but a rule could name `edge_types: [CALLS]`. Such a rule could not
   fail. It is now a configuration error, and `provider.edge_types` selects
   which relations to fetch.
3. **Documentation links counted as code dependencies.** GitNexus reports
   Markdown links as `IMPORTS` (`reason: markdown-link`). In another repository
   they were 70% of all `IMPORTS`. They are excluded by default.
4. **No measure of observation coverage.** A passing check said nothing about
   how much code the provider actually saw. Nodes now carry
   `observed_file_count`, and rules over mostly unobserved nodes warn.

## What each observation mode reports

Same rules, three provider settings:

| Mode | Violations | Assessment |
| --- | --- | --- |
| `IMPORTS` only (original design) | 0 | False all-clear: 13 architecture edges for 10k files |
| `+ CALLS, EXTENDS, IMPLEMENTS`, `min_confidence: 0.6` | 6 | 5 are false positives from `property-dispatch` |
| `+ exclude_reasons: [property-dispatch]` (committed config) | 1 | The one real finding, below |

### Real finding: the Rails backend layers form one cycle

`no-backend-layer-cycles` reports a strongly connected component of
`models`, `lib`, `jobs`, `services` and `policies`. Spot checks against the source
code confirm the edges:

- `lib/auto_wizard.rb` calls `TextModule.load` and `lib/auth/backend/ldap.rb` calls
  `LdapSource.by_user`, so `lib/` depends on models.
- Models depend on `lib/` 111 times, e.g. `app/models/ai/text_tool.rb` →
  `lib/user_info.rb`.
- `app/models/concerns/can_be_published.rb` calls `ScheduledTouchJob.touch_at`,
  and jobs call models back.

`lib/` is therefore not a lower layer under `app/`; it is entangled with the
domain model. The suggested cut makes this concrete. Removing 44 of the 202
observations, `lib -> models` (42), `lib -> services` (1) and
`models -> jobs` (1), leaves the layers `jobs > services > policies > models > lib`. `models-do-not-know-delivery` and `lib-does-not-know-delivery`
pass, so no observed model or `lib` code calls REST controllers.

One class of evidence is imprecise: `lib/knowledge_base/category/permission.rb`
reopens `class KnowledgeBase::Category`, and GitNexus merges both definitions, so
the model's `include` statements are attributed to the `lib` file. The coupling
(`lib` code nested in a model's namespace) is real, but the file-level evidence is
not.

### False positives removed by filtering

- `property-dispatch` (confidence 0.7): `shared/components/Form/Form.vue` calls
  `firstInput?.focus()` on a DOM element, and GitNexus links it to a `focus`
  function in `apps/desktop/.../CommonInputSearch.vue`. Every frontend violation
  (desktop ↔ mobile ↔ shared) was of this kind.
- `global-name-fallback` (confidence 0.5): for example `app/models/ticket.rb` →
  `spec/requests/knowledge_base/reorder_spec.rb`.

## Provider limits ArchGraph cannot fix

Share of files with no observed dependency at all, from the committed config:

| Node | Files | Unobserved | Cause |
| --- | ---: | ---: | --- |
| `legacy_ui` (CoffeeScript) | 814 | 99% | GitNexus does not parse CoffeeScript |
| `backend.graphql` | 593 | 96% | Ruby constant references not resolved |
| `backend.policies` | 174 | 94% | same |
| `backend.services` | 234 | 88% | same |
| `backend.models` | 618 | 66% | same |
| `frontend.desktop` | 1,763 | 44% | `#desktop/...` path aliases not resolved |

- **Rails autoloading.** Only 69 of 2,819 Ruby files under `app/` and `lib/` use
  `require`. The central `app/models/ticket.rb` has no `IMPORTS` edge. GitNexus
  logged 10,747 Ruby call sites it refused to resolve.
- **TypeScript path aliases.** The frontend has about 16,300 `import ... from`
  statements, of which about 10,000 use `#shared/`, `#desktop/`, `#mobile/` or
  `#tests/` aliases. GitNexus resolved about 1,900 TypeScript/Vue imports. In
  `TicketDetailView.vue` the relative import was resolved and the
  `#desktop/components/...` import was not. The aliases are declared in
  `tsconfig.base.json`, which `tsconfig.json` only `extends`. That GitNexus
  does not follow `extends` is a likely cause, but it was not confirmed by
  reindexing.

For these parts of zammad, a passing ArchGraph check is weak evidence. The
coverage warnings say so on every run.

## UI

`archgraph serve` rendered the backend and root focus levels with no browser
console errors. One readability problem remains: each relation kind between the
same two nodes is drawn as its own edge (`CALLS`, `IMPLEMENTS`, `IMPORTS`), so the
labels overlap on dense levels.
