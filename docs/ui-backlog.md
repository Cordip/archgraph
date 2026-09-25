# UI backlog

Planned work on the web UI (`archgraph serve`) that is not built yet, in no
fixed order. Each entry says what problem it solves; the design is open
unless stated.

## Merge files with identical dependency sets into one card

On test-heavy levels many files depend on exactly the same entries: in
lct-task3's Backend tests level, groups of test files each import the same
3 to 6 modules. Trunks (see the README) merge their wires, but every file
still takes a card of its own. Files at the same level whose sets of
dependencies (and of users) are identical could be drawn as one card, for
example "12 tests → core, ingest", that expands in place like a directory
group. Open questions: whether relation kinds must match too, how a
violation on one of the merged files is shown, and how the card is named
when the files share no directory.

## Level of detail when zoomed far out

Below about 20%, wires and trunks stay readable (they keep a minimum width
on screen), but card titles do not: a compact card's title can grow only as
far as the card is wide (up to 32 px, two lines), which is about 6 px on
screen at 20%. Two ideas were considered and not built. Collapsing each
directory group or row into one titled block at very low zoom would give
room for large titles, but it changes the layout, and so the routing, with
the zoom; the routing is expensive on large levels and a view that
rearranges itself while zooming is hard to follow. Titles drawn over the
cards regardless of their width would overlap their neighbours on dense
levels. A good answer probably labels groups rather than cards.

## Code viewer in a split right panel

The inspector already reserves a second, stacked pane
(`#pane-secondary` in `index.html`) for a file viewer: selecting a file or
an evidence line would show the source around that line next to the
details. This needs a read-only source endpoint limited to mapped files,
which the server deliberately does not have today (see "Human focus UI" in
the README), so the endpoint's scope and safety come first.

## `archgraph snapshot` with a live graph diff

A command that saves the compiled graph of the current revision, and a UI
mode that draws the live graph against a saved snapshot: entries and wires
added, removed or changed since then, and violations that appeared or went
away. It would show what a refactoring changed while it is in progress.

## Agent plan files drawn over the graph

A coding agent's plan (a file listing the moves, splits and dependency cuts
it intends) drawn as an overlay on the canvas: planned new entries, wires
to remove, wires to add, each linked to its step, so that a person can
review the plan against the architecture before the agent starts.
