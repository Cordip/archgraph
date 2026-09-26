---
name: boring-code
description: Write, refactor or review code so that it reads as if one careful person wrote all of it - visible hierarchy, one shape for every function, the same error handling everywhere, multi-line ifs, why-comments, shallow nesting, and detailed notes on optimised code. Use when writing new code, refactoring for readability, or reviewing code style in any language (examples in Rust, TypeScript and Python).
---

# Boring, readable code

The goal is code that a person reads once and understands: every file looks
as if the same careful author wrote it, in the spirit of early Go, where
there is one obvious way to do each thing. Boring is the compliment.
Architecture says where code belongs; this skill says how it reads.

## Scope

- **Formatters win on layout.** Where the project runs `rustfmt`,
  `prettier`, `black` or `ruff format`, keep their output; these rules are
  about structure, which formatters do not decide.
- **The project's conventions win on details.** Error types, naming and
  test layout follow what the repository already does. When a rule here
  conflicts with a written project rule, follow the project and say so.
- **Change what the task touches.** Apply the rules to code you write or
  change. Restyling untouched code is a separate change: its own commit,
  with no behaviour change, and only when the user asks for it.

## 1. Visible hierarchy

A reader should see how the author thought: what the entry point is, which
functions serve it, and what each helper is for.

- **Top-down order.** A file starts with a comment saying what it is for,
  then its public entry points, then the helpers in the order they are
  first used. The reader never meets a helper before the code that needs
  it.
- **Data first where data leads.** In Rust, and wherever types carry the
  design, put the types first, then what builds them, then what works on
  them: the hierarchy follows the data rather than the calls.
- **Names say the purpose.** `load_config`, `render_row`,
  `retry_with_backoff`; not `process`, `handle`, `do_it`, `helper2`. A
  function whose purpose needs "and" in its name is two functions.
- **One level of abstraction per function.** A function either
  orchestrates steps (calls with good names) or does one step in detail,
  not both.

```rust
//! Reads the architecture file and turns it into checked rules.

pub fn load(path: &Path) -> Result<Rules> { ... }   // entry point

fn parse(text: &str) -> Result<RawConfig> { ... }   // used by load, first
fn validate(raw: &RawConfig) -> Result<()> { ... }  // used by load, second
fn build_rules(raw: RawConfig) -> Rules { ... }     // used by load, last
```

## 2. One shape for every function

Every function has the same three parts, in this order, separated by a
blank line:

1. **Set up.** Check the inputs and return early when they are wrong;
   gather everything the work needs.
2. **Work.** Do the one thing the function exists for.
3. **Hand on.** Return the result, or pass it to the next step.

```typescript
export function summarize(orders: Order[], currency: string): Summary {
  if (orders.length === 0) {
    return EMPTY_SUMMARY;
  }
  const rate = exchangeRate(currency);

  let total = 0;
  for (const order of orders) {
    total += order.amount * rate;
  }

  return { count: orders.length, total, currency };
}
```

```python
def summarize(orders: list[Order], currency: str) -> Summary:
    if not orders:
        return EMPTY_SUMMARY
    rate = exchange_rate(currency)

    total = 0
    for order in orders:
        total += order.amount * rate

    return Summary(count=len(orders), total=total, currency=currency)
```

A function that does not fit this shape is doing more than one thing:
split it.

## 3. The same error handling everywhere

Pick the project's one way and use it in every file:

- **Rust:** return `Result` and propagate with `?`, adding context that
  says what was being done (`.with_context(|| format!("cannot read {}",
  path.display()))`). `unwrap` and `expect` only where a failure is a bug,
  with the reason in the `expect` message.
- **TypeScript:** throw `Error` (or the project's error class) with a
  message a person can act on; catch only at a boundary (a request handler,
  a UI event, `main`) where the error is reported.
- **Python:** raise a specific exception with an actionable message;
  catch the specific exception at a boundary, never a bare `except:`.

In every language:

- Never swallow an error. A `catch` that does nothing, or a fallback to a
  default that hides the failure, says "this worked" when it did not.
- The message says what failed and, where possible, what to do about it:
  `cannot read config.yaml: no such file; run init first`, not `error`.
- Errors are handled at the top of a function (the set-up part) or where
  they occur, with an early return; the happy path stays unindented.

## 4. Whitespace

- A blank line between the set-up, work and hand-on parts, and between
  steps of the work that a reader would name separately.
- A blank line between functions, types and top-level blocks.
- No blank line inside a step that belongs together.
- Never compress code to save lines: one statement per line, one
  declaration per line.

## 5. `if` is always multi-line

Every `if`, `else`, loop and guard has its body on its own lines, in braces
where the language has them, even when the body is one statement.

```rust
// No
if items.is_empty() { return Ok(()); }
let Some(user) = user else { return Err(anyhow!("no user")) };

// Yes
if items.is_empty() {
    return Ok(());
}
let Some(user) = user else {
    bail!("no user signed in");
};
```

```python
# No
if not items: return

# Yes
if not items:
    return
```

A conditional expression (`a ? b : c`, `b if a else c`, Rust's `if` as a
value) is allowed only to choose between two plain values on one line, and
never nested. Anything with a call that does work, or a second condition,
is an `if` statement.

## 6. Comments answer "why"

The code says what and how. A comment says what the code cannot:

- why this approach and not the obvious one;
- what would break if the code were changed ("sorted: the output must be
  byte-identical between runs");
- where a strange value comes from ("GitNexus truncates piped output at
  64 KiB, so write to a file");
- a link to the issue, specification or measurement behind a decision.

A comment that restates the code (`// increment i`) is noise; a comment
that is needed to explain what the code does means the code should be
clearer: better names, a smaller function. Keep comments true: change them
with the code.

## 7. Nesting no deeper than three levels

Inside a function body, at most three levels of indentation, four in rare,
explained cases. Get there with:

- **early return** for invalid input and finished cases;
- **`continue`** for items a loop skips;
- **a named function** for the inner block of a loop or a branch.

```typescript
// No: four levels
for (const file of files) {
  if (file.mapped) {
    for (const edge of file.edges) {
      if (edge.kind === "IMPORTS") {
        count += 1;
      }
    }
  }
}

// Yes
for (const file of files) {
  if (!file.mapped) {
    continue;
  }
  count += countImports(file.edges);
}
```

## 8. Boring and standard

- The standard library and the project's existing helpers before anything
  new; a new dependency only when it removes real work.
- Plain loops for anything with side effects or more than a couple of
  steps; a short `map`/`filter` chain is fine for a pure transformation.
- Explicit over implicit: no magic, no metaprogramming, no operator
  overloading or reflection unless the project already relies on it.
- One way to do each thing. If the codebase formats strings, builds paths
  or logs in one way, use that way everywhere.
- Constants with names instead of unexplained numbers and strings.
- Short is not the goal; obvious is. A few more lines that read straight
  down beat one clever line.

## 9. Optimised code is explained in detail

Where performance matters and the code is not the obvious version, put a
block comment before it, as the Go runtime does, that says:

1. **What it computes**, in one sentence.
2. **How the algorithm works**, step by step, with the invariants it keeps.
3. **Why it is written this way**: what the obvious version cost, measured
   (numbers, input size, machine), and what this one costs.
4. **What must stay true** for it to remain correct and fast, so the next
   person does not break it by "simplifying".

```typescript
// Culling: which items of a large drawing are near the view.
//
// A level can hold thousands of items and the view changes after every
// pan, so testing every item against the view grows with the whole level.
// Instead each item is put once into a grid of fixed-size cells, in every
// cell its bounding box touches; a query visits only the cells the view
// overlaps, so its cost grows with what is on screen. Items spanning
// several cells are deduplicated with a set.
//
// Measured: <the obvious version's cost> -> <this version's cost>, on
// <input and machine>. Write the real numbers; never estimate them.
//
// Invariant: an item's cells match its current box. Moving or resizing an
// item must rebuild its entry, or it vanishes when its old cells leave the
// view.
```

Keep the obvious version in a test as the reference, so the fast one is
checked against it.

## Review checklist

Before calling code done, read it top to bottom as a stranger would:

- [ ] The file opens with its purpose; entry points come before helpers.
- [ ] Every function has set-up, work and hand-on parts, with blank lines.
- [ ] Errors are handled the project's one way; nothing is swallowed.
- [ ] Every `if` and loop body is on its own lines.
- [ ] No function body is nested more than three levels.
- [ ] Every comment answers "why"; none restates the code.
- [ ] No clever line that needs a second reading.
- [ ] Optimised code has its algorithm, measurements and invariants written
      down.
- [ ] Formatter, linter and tests pass.
