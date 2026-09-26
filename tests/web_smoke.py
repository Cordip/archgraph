#!/usr/bin/env python3
"""Optional UI-only browser smoke test in an isolated DOM with mocked fetch/history interfaces.

The page runs on a fake origin so that browser storage (remembered layouts)
works; nothing is fetched from the network.

Requires Python Playwright and Chromium; does not run or validate the Rust server.
Set ARCHGRAPH_CHROMIUM to use a specific Chromium executable; otherwise
Playwright's own Chromium is used.
"""
from copy import deepcopy
import json
import os
import math
from pathlib import Path
import re

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[1]
NOTICE = "Only observed dependencies are checked. No observed edge is not proof of no runtime dependency."


def node(identity, title, children=(), files=(), kind="internal"):
    return {"id": identity, "title": title, "kind": kind,
            "description": "Fixture purpose; no Rust output is being simulated as validation.",
            "parent": identity.rsplit(".", 1)[0] if "." in identity else None,
            "children": list(children), "direct_files": list(files),
            "descendant_file_count": max(len(files), 2), "observed_file_count": 1, "interfaces": []}


NODES = {n["id"]: n for n in [
    node("app", "Application", ["app.api", "app.domain"]),
    node("app.api", "API", files=["src/api/a.rs"]),
    node("app.domain", "Domain", files=["src/domain/a.rs", "src/domain/b.rs"]),
    node("external", "External", ["external.service"], kind="external"),
    node("external.service", "Service", kind="external"),
    # More entries than the UI draws: listed in the table only.
    node("app.big", "Big", files=[f"src/big/f{i:03}.rs" for i in range(200)]),
    node("packages", "External packages", kind="external"),
    # Usage: entry points, files with no observed users, unindexed files.
    node("app.web", "Web", ["app.web.build", "app.web.ui", "app.web.old"]),
    node("app.web.build", "Build configuration", files=["web/vite.config.ts"]),
    node("app.web.ui", "Interface", files=["web/src/main.tsx", "web/src/App.tsx", "web/src/legacy.ts", "web/src/gen.ts"]),
    node("app.web.old", "Old widgets", files=["web/old/widget.ts"]),
]}
NODES["packages"].update(descendant_file_count=0, observed_file_count=0, descendant_package_count=1, packages=["package:python/ortools"])
NODES["app.web.build"].update(descendant_file_count=1, entry_point_count=1)
NODES["app.web.ui"].update(entry_point_count=1, no_observed_users_count=1, unindexed_file_count=1, outside_user_count=2)
NODES["app.web.old"].update(descendant_file_count=1, no_observed_users_count=1, no_outside_users=True, outside_user_count=0)
NODES["app.domain"]["interfaces"] = [{"name": "Domain service", "kind": "custom", "direction": "provides", "protocol": None, "contract": "invoice", "description": "Interface fixture"}]


def entry(identity, outside=False, file=None):
    n = NODES[identity]
    return {"id": f"file:{file}" if file else f"node:{identity}", "title": file or n["title"],
            "entry_kind": "file" if file else "architecture", "architecture_id": identity,
            "node_kind": None if file else n["kind"], "file_path": file,
            "file_count": 1 if file else n["descendant_file_count"],
            "description": n["description"], "interfaces": n["interfaces"],
            "outside_focus": outside, "violation_rule_ids": ["deny-api-domain"] if identity in ("app.api", "app.domain") else []}


EVIDENCE = {"from_file": "src/api/a.rs", "to_file": "src/domain/a.rs", "kind": "IMPORTS",
            "confidence": 0.9, "reason": '<img src=x onerror="window.__injected=1"> | literal provider text'}


def edge(source, target, manual=False, kind="IMPORTS"):
    return {"from": source, "to": target, "kind": "http" if manual else kind,
            "origin": "manual" if manual else "observed", "count": 1 if manual else 25,
            "evidence": [] if manual else [deepcopy(EVIDENCE)],
            "confidence_min": None if manual else 0.9, "confidence_max": None if manual else 0.9,
            "manual_edges": [{"id": "remote", "from": "app.api", "to": "external.service", "kind": "http", "label": "Remote API", "description": "Authored intent"}] if manual else [],
            "violation_rule_ids": [] if manual else ["deny-api-domain"]}


VIOLATION = {"rule_id": "deny-api-domain", "kind": "deny_dependency", "from": "app.api", "to": "app.domain",
             "edge_kind": "IMPORTS", "nodes": ["app.api", "app.domain"], "affected_nodes": ["app.api", "app.domain"],
             "count": 25, "message": "API to Domain is prohibited by this test-only rule.",
             "evidence": [EVIDENCE], "architecture_edges": [edge("app.api", "app.domain")]}
ORTOOLS = {"id": "package:python/ortools", "name": "ortools", "ecosystem": "python", "node": "packages",
           "imports": [{"file": "src/domain/a.rs", "line": 3, "specifier": "ortools.constraint_solver", "type_only": False, "node": "app.domain"},
                       {"file": "src/api/a.rs", "line": 9, "specifier": "ortools.sat", "type_only": True, "node": "app.api"}]}
PACKAGE_ENTRY = {"id": ORTOOLS["id"], "title": "ortools", "entry_kind": "package", "architecture_id": "packages",
                 "node_kind": None, "file_path": None, "file_count": 0, "observed_file_count": None,
                 "description": "Python package imported by 2 file(s).", "interfaces": [], "outside_focus": False,
                 "violation_rule_ids": [], "package": ORTOOLS}
# A wide name that fits a character count but not the card beside its glyph.
FONTSOURCE = {"id": "package:npm/@fontsource/ibm-plex-sans", "name": "@fontsource/ibm-plex-sans", "ecosystem": "npm", "node": "packages",
              "imports": [{"file": "src/api/a.rs", "line": 4, "specifier": "@fontsource/ibm-plex-sans/400.css", "type_only": False, "node": "app.api"}]}
WIDE_PACKAGE_ENTRY = {**PACKAGE_ENTRY, "id": FONTSOURCE["id"], "title": FONTSOURCE["name"],
                      "description": "npm package imported by 1 file(s).", "package": FONTSOURCE}
# An edge into a package carries package evidence, not a file.
PACKAGE_EDGE = {**edge("node:app.domain", ORTOOLS["id"]), "count": 1, "violation_rule_ids": [],
                "evidence": [{"from_file": "src/domain/a.rs", "to_file": ORTOOLS["id"], "kind": "IMPORTS", "confidence": 1.0, "reason": "package-import"}]}
def usage_file(identity, path, usage):
    return {**entry(identity, file=path), "violation_rule_ids": [], "usage": usage}


def usage_node(identity):
    n = NODES[identity]
    return {**entry(identity), "violation_rule_ids": [], "entry_point_count": n.get("entry_point_count", 0),
            "no_observed_users_count": n.get("no_observed_users_count", 0), "outside_user_count": n.get("outside_user_count", 1),
            "no_outside_users": n.get("no_outside_users", False)}


WEB_UI_FILES = [usage_file("app.web.ui", "web/src/main.tsx", "entry_point"), usage_file("app.web.ui", "web/src/App.tsx", "used"),
                usage_file("app.web.ui", "web/src/legacy.ts", "no_observed_users"), usage_file("app.web.ui", "web/src/gen.ts", "not_indexed")]
PROJECTIONS = {
    "app": {"focus": NODES["app"], "breadcrumbs": [NODES["app"]],
            "nodes": [entry("app.api"), entry("app.domain"), entry("external.service", True)],
            "edges": [edge("node:app.api", "node:app.domain"), edge("node:app.api", "node:app.domain", kind="CALLS"),
                      edge("node:app.api", "node:external.service", True)],
            "violations": [VIOLATION], "evidence_limit": 20, "evidence_notice": NOTICE,
            "layers": [["node:app.api"], ["node:app.domain"]]},
    "app.domain": {"focus": NODES["app.domain"], "breadcrumbs": [NODES["app"], NODES["app.domain"]],
                   "nodes": [entry("app.domain", file="src/domain/a.rs"), entry("app.domain", file="src/domain/b.rs"), entry("app.api", True)],
                   "edges": [edge("node:app.api", "file:src/domain/a.rs")],
                   "violations": [VIOLATION], "evidence_limit": 20, "evidence_notice": NOTICE},
    "app.big": {"focus": NODES["app.big"], "breadcrumbs": [NODES["app"], NODES["app.big"]],
                "nodes": [entry("app.big", file=f) for f in NODES["app.big"]["direct_files"]],
                "edges": [edge("file:src/big/f000.rs", "file:src/big/f001.rs")],
                "violations": [], "evidence_limit": 20, "evidence_notice": NOTICE, "layers": []},
    "packages": {"focus": NODES["packages"], "breadcrumbs": [NODES["packages"]],
                 "nodes": [PACKAGE_ENTRY, WIDE_PACKAGE_ENTRY, entry("app.api", True), entry("app.domain", True)],
                 "edges": [PACKAGE_EDGE, edge("node:app.api", ORTOOLS["id"])],
                 "violations": [], "evidence_limit": 20, "evidence_notice": NOTICE, "layers": [[ORTOOLS["id"], FONTSOURCE["id"]]]},
    "app.web": {"focus": NODES["app.web"], "breadcrumbs": [NODES["app"], NODES["app.web"]],
                "nodes": [usage_node("app.web.build"), usage_node("app.web.old"), usage_node("app.web.ui")],
                "edges": [], "violations": [], "evidence_limit": 20, "evidence_notice": NOTICE, "layers": []},
    "app.web.ui": {"focus": NODES["app.web.ui"], "breadcrumbs": [NODES["app"], NODES["app.web"], NODES["app.web.ui"]],
                   "nodes": WEB_UI_FILES, "edges": [{**edge("file:web/src/main.tsx", "file:web/src/App.tsx"), "violation_rule_ids": []}],
                   "violations": [], "evidence_limit": 20, "evidence_notice": NOTICE, "layers": []},
}
PACKAGES = [{"id": ORTOOLS["id"], "name": "ortools", "ecosystem": "python", "node": "packages", "file_count": 2, "nodes": ["app.api", "app.domain"],
             "imports": ORTOOLS["imports"]}]
# Source text for the code viewer (GET /api/source). The evidence file
# imports its target on line 150, far enough down to need scrolling.
API_SOURCE = "\n".join(["use crate::domain::a;" if i == 150 else f"// filler line {i}: nothing names the target here" for i in range(1, 301)]) + "\n"
SOURCES = {"src/api/a.rs": API_SOURCE,
           "src/domain/a.rs": "// Domain model\nuse std::fmt;\nuse ortools::constraint_solver; // line 3\n\npub struct Invoice {\n    pub total: u64,\n}\n",
           "web/src/App.tsx": "import { main } from './main';\n\nexport function App(): string {\n  return \"app\"; // a comment\n}\n",
           # Markup: a Vue component's sections, and Ruby holes in ERB, even
           # inside an attribute value and over several lines.
           "web/src/Card.vue": "<template>\n  <div class=\"card\" :title=\"label\">\n    <!-- a note\n         over two lines -->\n"
                               "    <span>{{ label }}</span>\n  </div>\n</template>\n\n<script setup lang=\"ts\">\n"
                               "import { computed } from \"vue\";\nconst label = computed(() => \"card\"); // shown\n</script>\n\n"
                               "<style scoped>\n.card { color: #333; }\n</style>\n",
           "app/views/cards/show.html.erb": "<h1 class=\"title\"><%= @card.title %></h1>\n<% if @card.done? %>\n  <p>Done</p>\n<% end %>\n"
                                            "<%# a note for the template %>\n<a href=\"<%= card_path(@card) %>\">Open</a>\n<%\n"
                                            "  total = @card.items.sum(:price) # in cents\n%>\n"}
# Snapshots (GET /api/snapshots) and what changed since `before`, level by
# level (GET /api/diff/{node}), in the live levels' entry IDs.
SNAPSHOTS = {"enabled": True, "snapshots": [{"name": "before", "commit": "5b29b17c0ffee"}, {"name": "old.2", "commit": None}]}


def level_diff(focus, **changes):
    empty = {"files_added": [], "files_removed": [], "files_moved": [], "files_reassigned": [],
             "dependencies_added": [], "dependencies_removed": [], "violations_appeared": [], "violations_resolved": []}
    diff = {"snapshot": "before", "commit": "5b29b17c0ffee", "focus": focus, "focus_existed": True,
            "entries_added": [], "entries_removed": [], "entries_moved": [], "entries_resized": [],
            "edges_added": [], "edges_removed": [], "edges_changed": [], "violations_appeared": [], "violations_resolved": [],
            "summary": {**empty, "snapshot": "before", "commit": "5b29b17c0ffee", "scope": focus}, "revision": 1, "warning": None}
    summary = changes.pop("summary", {})
    diff.update(changes)
    diff["summary"].update(summary)
    return diff


def diff_edge(source, target, kind="IMPORTS", origin="observed", before=None, after=None):
    item = {"from": source, "to": target, "kind": kind, "origin": origin}
    if before is not None:
        item.update(count_before=before, count_after=after)
    return item


DIFFS = {
    # The external service is new, with its (manual) wire; the API grew by a
    # file; API → Domain gained CALLS and IMPORTS grew from 10 to 25; the
    # Legacy node is gone, with its wire into Domain and one from Domain
    # back to the API.
    "app": level_diff("app", entries_added=["node:external.service"], entries_resized=[{"id": "node:app.api", "file_count_before": 1, "file_count_after": 2}],
                      entries_removed=[{"id": "node:app.legacy", "title": "Legacy", "entry_kind": "architecture", "file_path": None, "architecture_id": "app.legacy", "outside_focus": False}],
                      edges_added=[diff_edge("node:app.api", "node:app.domain", "CALLS"), diff_edge("node:app.api", "node:external.service", "http", "manual")],
                      edges_changed=[diff_edge("node:app.api", "node:app.domain", before=10, after=25)],
                      edges_removed=[diff_edge("node:app.legacy", "node:app.domain", before=7, after=0), diff_edge("node:app.domain", "node:app.api", "CALLS", before=3, after=0)],
                      violations_appeared=[{"rule_id": "deny-api-domain", "from": "app.api", "to": "app.domain", "edge_kind": "IMPORTS", "nodes": []}],
                      violations_resolved=[{"rule_id": "legacy-is-frozen", "from": "app.legacy", "to": "app.domain", "edge_kind": "IMPORTS", "nodes": []}],
                      summary={"files_moved": [{"from": "src/domain/old_a.rs", "to": "src/domain/a.rs", "node_before": "app.domain", "node_after": "app.domain"}],
                               "files_added": [{"path": "src/api/a.rs", "node": "app.api"}],
                               "files_removed": [{"path": "src/legacy/x.rs", "node": "app.legacy"}]}),
    # A file renamed since the snapshot keeps its identity.
    "app.domain": level_diff("app.domain", entries_moved=[{"from": "file:src/domain/old_a.rs", "to": "file:src/domain/a.rs"}],
                             entries_added=["file:src/domain/b.rs"]),
    # A level the snapshot did not have: everything on it is new.
    "app.web": level_diff("app.web", focus_existed=False, entries_added=["node:app.web.build", "node:app.web.old", "node:app.web.ui"]),
    # Any other level: nothing changed.
    "": level_diff(""),
}
META = {"project": {"name": "Browser fixture", "root": "app"}, "provider": {"provider": "fixture"}, "stats": {},
        "schema_version": 1, "evidence_notice": NOTICE, "diagnostics": ["Test-only coverage warning"], "read_only": True}


def wires_projection():
    """A level with fan-out, skipped rows, an upward edge and two kinds on one edge."""
    files = [f"w/{name}.ts" for name in "abcdefg"]
    pairs = [("a", "b"), ("a", "c"), ("a", "d"), ("a", "g"), ("b", "e"), ("c", "e"), ("c", "f"),
             ("d", "f"), ("e", "g"), ("f", "g"), ("g", "a"), ("d", "e")]
    edges = [{**edge(f"file:w/{a}.ts", f"file:w/{b}.ts"), "violation_rule_ids": []} for a, b in pairs]
    edges.append({**edge("file:w/a.ts", "file:w/b.ts", kind="CALLS"), "violation_rule_ids": []})
    return {"focus": {**NODES["app.big"], "id": "app.wires", "title": "Wires"}, "breadcrumbs": [NODES["app"]],
            "nodes": [{**entry("app.big", file=f), "violation_rule_ids": []} for f in files],
            "edges": edges, "violations": [], "evidence_limit": 20, "evidence_notice": NOTICE, "layers": []}


def trunk_projection():
    """Six files in three rows, all depending on one outside entry; one of those wires violates a rule."""
    files = [f"t/s{i}.ts" for i in range(6)]
    edges = [{**edge(f"file:{f}", "node:external.service"), "violation_rule_ids": ["deny-api-domain"] if i == 2 else []} for i, f in enumerate(files)]
    edges += [{**edge("file:t/s0.ts", "file:t/s1.ts"), "violation_rule_ids": []}, {**edge("file:t/s1.ts", "file:t/s2.ts"), "violation_rule_ids": []}]
    return {"focus": {**NODES["app.big"], "id": "app.trunks", "title": "Trunks"}, "breadcrumbs": [NODES["app"]],
            "nodes": [{**entry("app.big", file=f), "violation_rule_ids": []} for f in files] + [entry("external.service", True)],
            "edges": edges, "violations": [], "evidence_limit": 20, "evidence_notice": NOTICE, "layers": []}


def dense_projection():
    """Twelve files, each depending on every later one: 66 wires, one of them violating."""
    files = [f"d/f{i:02}.ts" for i in range(12)]
    edges = [{**edge(f"file:{files[i]}", f"file:{files[j]}"), "violation_rule_ids": ["deny-api-domain"] if (i, j) == (0, 11) else []}
             for i in range(12) for j in range(i + 1, 12)]
    return {"focus": {**NODES["app.big"], "id": "app.dense", "title": "Dense"}, "breadcrumbs": [NODES["app"]],
            "nodes": [{**entry("app.big", file=f), "violation_rule_ids": []} for f in files],
            "edges": edges, "violations": [], "evidence_limit": 20, "evidence_notice": NOTICE, "layers": []}


# Segments of every drawn wire, from the path data (boards draw only M and L).
SEGMENTS = """() => [...document.querySelectorAll('#graph .edge-line, #graph .trunk-strand')].filter((line) => line.getAttribute('d')).map((line) => {
    const numbers = line.getAttribute('d').match(/-?[\\d.]+/g).map(Number);
    const points = [];
    for (let i = 0; i + 1 < numbers.length; i += 2) points.push([numbers[i], numbers[i + 1]]);
    return points;
})"""


# Every wire's drawing, in drawing order, to compare two layouts.
WIRES = """() => [...document.querySelectorAll('#graph .edge-line, #graph .edge-gap, #graph .trunk-strand, #graph .trunk-casing, #graph .trunk-arrow, #graph .edge-label')].map((e) => [e.getAttribute('class'), e.getAttribute('d') || `${e.getAttribute('x')},${e.getAttribute('y')}`])"""

# Watches the drawing while an entry is dragged: mutations inside #graph,
# animation frames, and calls to the layout functions and the entry's moves.
DRAG_WATCH = """() => { const d = window.__drag = { mutations: 0, frames: 0, run: true, calls: {} };
    const loop = () => { if (!d.run) return; d.frames++; requestAnimationFrame(loop); };
    requestAnimationFrame(loop);
    new MutationObserver((list) => { if (d.run) d.mutations += list.length; }).observe(document.getElementById('graph'), { subtree: true, childList: true, attributes: true });
    // An entry's move is dragTo (or moveEntry, before moves waited for a frame).
    for (const [name, counted] of [['route', 'route'], ['layoutTrunk', 'layoutTrunk'], ['drawScene', 'drawScene'], ['dragTo', 'moves'], ['moveEntry', 'moves']]) {
        if (!window[name] || window['__plain_' + name]) continue;
        const f = window['__plain_' + name] = window[name];
        window[name] = function (...a) { window.__drag.calls[counted] = (window.__drag.calls[counted] || 0) + 1; return f.apply(this, a); };
    } }"""

# Four pointer moves in each of ten frames, as a fast mouse sends them.
DRAG_BURST = """async ([x, y]) => { const stage = document.getElementById('stage');
    for (let frame = 0; frame < 10; frame++) {
        for (let i = 0; i < 4; i++) stage.dispatchEvent(new PointerEvent('pointermove', { pointerId: 1, pointerType: 'mouse', clientX: x + frame * 12 + i * 3, clientY: y + frame * 5, bubbles: true }));
        await new Promise((resolve) => requestAnimationFrame(resolve));
    } }"""


def drag_checks(page, selector, key, board):
    """Drags an entry: during the drag the drawing (#graph) does not change,
    the entry moves at most once a frame and nothing is laid out; the drop
    lays the level out once, exactly as drawing it afresh would, and stores
    the position. Escape during a drag puts the entry back."""
    card = page.locator("#graph " + selector)
    before = card.get_attribute("transform")
    box = card.bounding_box()
    start = page.evaluate("(s) => { const id = document.querySelector('#graph ' + s).dataset.id; return { ...scene.positions.get(id), k: camera.k, id }; }", selector)
    x, y = box["x"] + 30, box["y"] + 15
    page.mouse.move(x, y)
    page.mouse.down()
    page.mouse.move(x + 10, y + 4)
    page.wait_for_timeout(50)
    page.evaluate(DRAG_WATCH)
    page.evaluate(DRAG_BURST, [x + 12, y + 6])
    last = (x + 160, y + 70)
    page.mouse.move(*last, steps=4)
    page.wait_for_timeout(100)
    watch = page.evaluate("() => { window.__drag.run = false; return window.__drag; }")
    calls = watch["calls"]
    assert watch["mutations"] == 0, watch
    assert 0 < calls.get("moves", 0) <= watch["frames"], watch
    assert not any(calls.get(name) for name in ("route", "layoutTrunk", "drawScene")), watch
    assert page.locator("#lift-graph " + selector).count() == 1 and page.locator("#graph " + selector).count() == 0
    # Meanwhile its wires take simple courses: straight on curves, one
    # elbow or a Z on a board.
    courses = page.evaluate("""() => [...document.querySelectorAll('#lift-graph .edge-line:not(.edge-end)')].map((line) => {
        const numbers = line.getAttribute('d').match(/-?[\\d.]+/g).map(Number), points = [];
        for (let i = 0; i + 1 < numbers.length; i += 2) points.push([numbers[i], numbers[i + 1]]);
        return [line.getAttribute('d').replace(/[^A-Z]/g, ''), points]; })""")
    assert courses, "the dragged entry's wires are on the lift layer"
    for commands, points in courses:
        assert set(commands) <= {"M", "L"} and len(points) <= (4 if board else 2), (commands, points)
        assert not board or all(abs(p[0] - q[0]) < 0.6 or abs(p[1] - q[1]) < 0.6 for p, q in zip(points, points[1:])), points
    page.evaluate("() => { window.__drag.calls = {}; }")
    page.mouse.up()
    page.wait_for_timeout(400)
    calls = page.evaluate("window.__drag.calls")
    assert calls.get("drawScene") == 1 and (board or calls.get("route") == 1), calls
    assert page.locator("#graph " + selector).count() == 1 and page.locator("#viewport.entry-drag").count() == 0
    assert card.get_attribute("transform") != before
    saved = page.evaluate("(k) => JSON.parse(localStorage.getItem(Object.keys(localStorage).find((key) => key.startsWith(k))))", key)
    if not board:
        # The position of the last pointer move, as a drag always stored it.
        expected = [round(start["x"] + (last[0] - x) / start["k"]), round(start["y"] + (last[1] - y) / start["k"])]
        assert saved[start["id"]] == expected, (saved, expected)
    else:
        assert start["id"] in saved, saved
    wires = page.evaluate(WIRES)
    page.evaluate("redraw()")
    page.wait_for_timeout(100)
    assert page.evaluate(WIRES) == wires, "the drop draws what a fresh layout draws"
    # Escape during a drag: the entry goes back and nothing is stored.
    moved = card.get_attribute("transform")
    stored_before = page.evaluate("() => JSON.stringify(Object.entries(localStorage).filter(([key]) => key.includes('layout')).sort())")
    box = card.bounding_box()
    page.mouse.move(box["x"] + 30, box["y"] + 15)
    page.mouse.down()
    page.mouse.move(box["x"] + 130, box["y"] + 65, steps=5)
    page.wait_for_timeout(50)
    page.keyboard.press("Escape")
    page.mouse.up()
    page.wait_for_timeout(300)
    assert card.get_attribute("transform") == moved and page.locator("#graph " + selector).count() == 1
    assert page.evaluate("() => JSON.stringify(Object.entries(localStorage).filter(([key]) => key.includes('layout')).sort())") == stored_before
    assert page.evaluate(WIRES) == wires


def angles_ok(page, step):
    bad = []
    for points in page.evaluate(SEGMENTS):
        for (x1, y1), (x2, y2) in zip(points, points[1:]):
            if abs(x2 - x1) + abs(y2 - y1) < 0.5:
                continue
            angle = math.degrees(math.atan2(y2 - y1, x2 - x1)) % 180
            off = min(angle % step, step - angle % step)
            # Points are rounded to 0.1 px: short hop ramps may be off by a degree.
            if off > (2.5 if math.hypot(x2 - x1, y2 - y1) < 12 else 0.6):
                bad.append(((x1, y1), (x2, y2), angle))
    return bad


def spacing_problems(page, pitch):
    """Pairs of parallel horizontal or vertical runs of different wires closer than the pitch.

    Wires of one trunk share their tracks on purpose, so they are not compared."""
    cores = page.evaluate("() => scene.edgeEls.map((item) => item.core)")
    trunks = page.evaluate("() => scene.edgeEls.map((item) => item.trunk)")
    runs = []
    for index, core in enumerate(cores):
        for (x1, y1), (x2, y2) in zip([(p["x"], p["y"]) for p in core], [(p["x"], p["y"]) for p in core[1:]]):
            if abs(y1 - y2) < 0.01 and abs(x2 - x1) > 0.5:
                runs.append((index, "h", y1, min(x1, x2), max(x1, x2)))
            elif abs(x1 - x2) < 0.01 and abs(y2 - y1) > 0.5:
                runs.append((index, "v", x1, min(y1, y2), max(y1, y2)))
    problems = []
    for i, a in enumerate(runs):
        for b in runs[i + 1:]:
            if a[0] == b[0] or a[1] != b[1] or (trunks[a[0]] and trunks[a[0]] == trunks[b[0]]):
                continue
            overlap = min(a[4], b[4]) - max(a[3], b[3])
            if overlap > 1 and abs(a[2] - b[2]) < pitch - 0.5:
                problems.append((a, b))
    return problems


def camera(page):
    """The canvas camera as (x, y, k), parsed from the #graph transform (the
    drawing starts half a canvas above and left of the canvas: overscan)."""
    numbers = [float(n) for n in re.findall(r"-?[\d.]+", page.locator("#graph").get_attribute("transform"))]
    box = page.locator("#canvas").bounding_box()
    return numbers[0] - box["width"] / 2, numbers[1] - box["height"] / 2, numbers[2]


def canvas_place(page, selector):
    """Where an element's centre is in the canvas, as fractions of its size."""
    c = page.locator("#canvas").bounding_box()
    b = page.locator(selector).bounding_box()
    return {"fx": (b["x"] + b["width"] / 2 - c["x"]) / c["width"], "fy": (b["y"] + b["height"] / 2 - c["y"]) / c["height"],
            "width": c["width"], "height": c["height"]}


def canvas_centre(page):
    """The drawing's point at the canvas's centre, in drawing units."""
    x, y, k = camera(page)
    box = page.locator("#canvas").bounding_box()
    return (box["width"] / 2 - x) / k, (box["height"] / 2 - y) / k


def main():
    checks = []
    with sync_playwright() as playwright:
        # Without ARCHGRAPH_CHROMIUM, Playwright's own Chromium is used
        # (`python -m playwright install chromium`).
        executable = os.environ.get("ARCHGRAPH_CHROMIUM") or None
        browser = playwright.chromium.launch(executable_path=executable, headless=True, args=["--no-sandbox"])
        page = browser.new_page(viewport={"width": 1440, "height": 1000})
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.route("https://archgraph.invalid/**", lambda route: route.fulfill(body="<!doctype html><title>fixture</title>", content_type="text/html"))
        page.goto("https://archgraph.invalid/")
        mocked = {"meta": META, "projections": PROJECTIONS, "nodes": list(NODES.values()), "violations": [VIOLATION], "packages": PACKAGES, "sources": SOURCES,
                  "snapshots": SNAPSHOTS, "diffs": DIFFS}
        index = (ROOT / "src/web/index.html").read_text()
        index = re.sub(r'<script[^>]*>.*?</script>', '', index, flags=re.S)
        index = re.sub(r'<link[^>]*rel="stylesheet"[^>]*>', '', index)
        page.set_content(index)
        page.add_style_tag(content=(ROOT / "src/web/style.css").read_text())
        page.evaluate("(() => { window.__fixture = " + json.dumps(mocked) + "; const fixture = window.__fixture;" + r"""
            window.__history = [];
            window.history.pushState = (state, title, url) => window.__history.push(String(url));
            window.fetch = async (input) => {
                const url = new URL(input, 'https://archgraph.invalid/');
                let payload = null;
                if (url.pathname === '/api/meta') payload = fixture.meta;
                else if (url.pathname === '/api/nodes') payload = fixture.nodes;
                else if (url.pathname === '/api/violations') payload = {violations: fixture.violations};
                else if (url.pathname === '/api/packages') payload = {enabled: true, packages: fixture.packages};
                else if (url.pathname.startsWith('/api/focus/')) payload = fixture.projections[decodeURIComponent(url.pathname.slice('/api/focus/'.length))];
                else if (url.pathname === '/api/source') {
                    // Like the server: mapped files only, the text left out
                    // while the caller's hash still matches.
                    const path = url.searchParams.get('path');
                    window.__sourceFetches = (window.__sourceFetches || 0) + 1;
                    if (path === 'src/big/huge.ts' && !fixture.sources[path]) {
                        const lines = [];
                        for (let i = 1; i <= 20000; i++) lines.push(`export const value${i} = compute("row ${i}", ${i} * 2); // note ${i}`);
                        fixture.sources[path] = lines.join('\n') + '\n';
                    }
                    const text = fixture.sources[path];
                    if (text === undefined) return new Response(JSON.stringify({error: `\`${path}\` is not a mapped file of this architecture (excluded, outside the source roots or unknown); only mapped files are served`, reason: 'unmapped'}), {status: 403, headers: {'Content-Type': 'application/json'}});
                    let hash = 0;
                    for (let i = 0; i < text.length; i++) hash = (hash * 31 + text.charCodeAt(i)) | 0;
                    hash = String(hash >>> 0);
                    const unchanged = url.searchParams.get('if_hash') === hash;
                    payload = {path, node: 'app.fixture', line_count: text.split('\n').length - 1, bytes: text.length, hash, unchanged};
                    if (!unchanged) payload.text = text;
                }
                else if (url.pathname === '/api/snapshots') payload = fixture.snapshots;
                else if (url.pathname.startsWith('/api/diff/')) {
                    // Like the server: a snapshot that is not saved is refused.
                    window.__diffFetches = (window.__diffFetches || 0) + 1;
                    const name = url.searchParams.get('snapshot') || 'before';
                    if (!fixture.snapshots.snapshots.some((item) => item.name === name)) return new Response(JSON.stringify({error: `no snapshot named \`${name}\`; take one with \`archgraph snapshot --name ${name}\``, reason: 'missing'}), {status: 404, headers: {'Content-Type': 'application/json'}});
                    const focus = decodeURIComponent(url.pathname.slice('/api/diff/'.length));
                    // Levels without changes of their own compare as unchanged.
                    const diff = fixture.diffs[focus] || (fixture.projections[focus] ? {...fixture.diffs[''], focus} : null);
                    payload = diff && {...diff, snapshot: name};
                }
                else if (url.pathname === '/api/search') {
                    const q = (url.searchParams.get('q') || '').toLowerCase();
                    payload = fixture.nodes.filter(n => n.id.toLowerCase().includes(q) || n.title.toLowerCase().includes(q));
                }
                return new Response(JSON.stringify(payload || {error: 'unknown architecture node; use search'}),
                    {status: payload ? 200 : 404, headers: {'Content-Type': 'application/json'}});
            };
        })()
        """)
        page.add_script_tag(content=(ROOT / "src/web/board.js").read_text())
        page.add_script_tag(content=(ROOT / "src/web/dsm.js").read_text())
        page.add_script_tag(content=(ROOT / "src/web/viewer.js").read_text())
        page.add_script_tag(content=(ROOT / "src/web/app.js").read_text())
        page.wait_for_function("document.getElementById('focus-title').textContent === 'Application'")
        assert page.locator("#graph .node").count() == 3
        # Two relation kinds between the same pair are drawn as one edge.
        assert page.locator("#graph .edge").count() == 2
        assert page.locator("#graph .edge-line[marker-end]").count() == 2
        assert any(label.startswith("2 kinds × 50") for label in page.locator("#graph .edge-label").all_text_contents())
        assert page.locator("#graph .violating").count() >= 1
        # Entries in a violation carry a revision cloud, a shape and not only a colour.
        assert page.locator("#graph .node.violating .cloud").count() == 2
        # Layered layout: the dependent (API) is drawn above its dependency.
        api = page.get_by_role("button", name="API", exact=True).bounding_box()
        domain = page.get_by_role("button", name="Domain", exact=True).bounding_box()
        assert api["y"] < domain["y"], (api, domain)
        # Nothing selected: the details panel describes the level.
        assert "This level" in page.locator("#details").inner_text()
        assert NOTICE in page.locator("#evidence-notice").inner_text()
        checks.append("root projection, directed arrows, merged relation kinds, external/manual entries, violation markers, level overview")
        # Comparing with a snapshot is off by default: nothing is asked for
        # and nothing extra is drawn or listed.
        assert page.evaluate("window.__diffFetches || 0") == 0
        assert page.locator("#graph .rev, #graph .ghost-card, #graph .edge.ghost, #legend .legend-subhead, #compare-section").count() == 0
        assert page.locator("#compare-label").inner_text() == "Compare" and page.locator("#compare-count").is_hidden()
        assert "compare" not in page.evaluate("location.search")
        checks.append("comparing with a snapshot is off by default: no diff is fetched and nothing extra is drawn")

        page.locator("#graph .edge-label", has_text="2 kinds").click()
        assert "src/api/a.rs" in page.locator("#details").inner_text()
        assert "CALLS × 25" in page.locator("#details").inner_text()
        assert "IMPORTS × 25" in page.locator("#details").inner_text()
        assert "<img src=x" in page.locator("#details").inner_text()
        assert page.locator("#details img").count() == 0
        assert page.evaluate("window.__injected") is None
        assert page.locator("script").count() == 4
        checks.append("edge evidence and hostile provider text rendered without HTML execution")

        # The canvas is one tab stop: arrow keys move between entries, Enter
        # shows details, whose dependency list reaches the edges by keyboard.
        assert page.locator("#graph .node[tabindex='0']").count() == 1
        page.locator("#graph .node[tabindex='0']").focus()
        start = page.evaluate("document.activeElement.getAttribute('aria-label')")
        page.keyboard.press("ArrowDown")
        moved = page.evaluate("document.activeElement.getAttribute('aria-label')")
        assert moved != start and page.locator("#graph .node[tabindex='0']").count() == 1, (start, moved)
        page.mouse.move(0, 0)
        page.get_by_role("button", name="API", exact=True).focus()
        page.keyboard.press("Enter")
        assert "Depends on (2)" in page.locator("#details").inner_text()
        # Selecting an entry dims everything it is not connected to: the
        # drawing's layer is dimmed and copies of the lit entries and wires
        # are drawn over it, while the drawing itself is left alone.
        # (The focused entry lights the same neighbourhood as a hover.)
        assert page.locator("#viewport.lifted").count() == 1 and page.evaluate("selection.nodes.size") == 3
        assert page.locator("#lift-graph .node").count() == 3
        assert page.locator("#graph .lit, #graph .hover, #graph.has-selection, #graph.has-hover").count() == 0
        # A row in the details and its entry on the canvas light each other.
        row = page.locator("#details .dependency", has_text="app.domain")
        row.hover()
        assert page.locator("#lift-graph.has-hover").count() == 1
        assert page.locator("#lift-graph .node.hover").evaluate_all("els => els.map(e => e.getAttribute('aria-label')).sort()") == ["API", "Domain"]
        page.mouse.move(0, 0)
        assert page.locator("#lift-graph.has-hover").count() == 0 and page.locator("#lift-graph.has-selection").count() == 1
        page.get_by_role("button", name="Domain", exact=True).hover()
        assert "linked" in row.get_attribute("class")
        assert page.locator("#details .dependency.linked").count() == 1
        page.mouse.move(0, 0)
        assert page.locator("#details .dependency.linked").count() == 0
        row.click()
        assert "CALLS × 25" in page.locator("#details").inner_text()
        assert "Observed dependency" in page.locator("#details").inner_text()
        page.keyboard.press("Escape")
        assert "This level" in page.locator("#details").inner_text()
        checks.append("keyboard navigation between entries, highlighted neighbourhood, dependency rows and canvas entries light each other on hover, dependency list to edge evidence, Escape back to the level")

        # Canvas: drag empty space to pan, wheel to zoom at the pointer.
        stage = page.locator("#canvas").bounding_box()
        x0, y0, k0 = camera(page)
        page.mouse.move(stage["x"] + 20, stage["y"] + stage["height"] / 2)
        page.mouse.down()
        page.mouse.move(stage["x"] + 140, stage["y"] + stage["height"] / 2 + 50, steps=6)
        page.mouse.up()
        x1, y1, k1 = camera(page)
        assert (round(x1 - x0), round(y1 - y0), k1) == (120, 50, k0), (x0, y0, x1, y1)
        px, py = 500, 400
        world = ((px - x1) / k1, (py - y1) / k1)
        page.mouse.move(stage["x"] + px, stage["y"] + py)
        page.mouse.wheel(0, -240)
        # During the gesture only the stage layer moves; the drawing takes
        # the new camera once the wheel pauses.
        assert page.locator("#viewport").evaluate("e => e.style.transform") != ""
        page.wait_for_timeout(300)
        assert page.locator("#viewport").evaluate("e => e.style.transform") == ""
        x2, y2, k2 = camera(page)
        assert k2 > k1 * 1.3, (k1, k2)
        assert abs((px - x2) / k2 - world[0]) < 0.5 and abs((py - y2) / k2 - world[1]) < 0.5
        assert page.locator("#zoom-level").inner_text() == f"{round(k2 * 100)}%"
        # A pan of 45% of the canvas is still one moving picture, and the
        # drawing is rendered far enough beyond the canvas (overscan) that no
        # blank strip shows at its edge.
        empty = page.evaluate("""() => { const c = document.getElementById('canvas').getBoundingClientRect();
            for (let y = c.top + c.height * 0.3; y < c.bottom - 60; y += 23) for (let x = c.right - 60; x > c.left + c.width * 0.6; x -= 29)
                if (document.elementFromPoint(x, y).id === 'grid-bg') return [x, y]; return null; }""")
        page.mouse.move(*empty)
        page.mouse.down()
        page.mouse.move(empty[0] - stage["width"] * 0.45, empty[1] - 10, steps=8)
        cover = page.evaluate("""() => { const s = document.getElementById('stage').getBoundingClientRect(), c = document.getElementById('canvas').getBoundingClientRect();
            return [document.getElementById('viewport').style.transform, s.left <= c.left + 0.5 && s.top <= c.top + 0.5 && s.right >= c.right - 0.5 && s.bottom >= c.bottom - 0.5]; }""")
        page.mouse.up()
        assert cover[0] != "" and cover[1], cover
        page.locator("#zoom-fit").click()
        page.wait_for_timeout(500)
        checks.append("pan by dragging empty space, wheel zoom anchored at the pointer, zoom to fit")

        # Entries can be dragged; the position is remembered and can be reset.
        service = page.get_by_role("button", name="Service", exact=True)
        original = service.get_attribute("transform")
        box = service.bounding_box()
        page.mouse.move(box["x"] + 30, box["y"] + 20)
        page.mouse.down()
        page.mouse.move(box["x"] + 190, box["y"] + 90, steps=8)
        page.mouse.up()
        assert service.get_attribute("transform") != original
        assert page.evaluate("Object.keys(localStorage).some(k => k.startsWith('archgraph.layout.v1:Browser fixture:app'))")
        assert "This level" in page.locator("#details").inner_text(), "a drag is not a click"
        page.locator("#reset-layout").click()
        page.wait_for_timeout(500)
        assert service.get_attribute("transform") == original
        # With Space held, dragging over an entry pans instead of moving it.
        page.locator("#canvas").focus()
        box = service.bounding_box()
        cx, cy, ck = camera(page)
        page.mouse.move(box["x"] + 30, box["y"] + 20)
        page.keyboard.down("Space")
        page.mouse.down()
        page.mouse.move(box["x"] + 110, box["y"] + 50, steps=5)
        page.mouse.up()
        page.keyboard.up("Space")
        nx, ny, nk = camera(page)
        assert (round(nx - cx), round(ny - cy)) == (80, 30) and service.get_attribute("transform") == original
        checks.append("node drag, remembered layout, reset layout, Space-drag pans over entries")
        drag_checks(page, ".node[data-id='node:external.service']", "archgraph.layout.v1:Browser fixture:app", False)
        page.locator("#reset-layout").click()
        page.wait_for_timeout(500)
        checks.append("a dragged entry and its wires move on the lift layer, at most once a frame, without changing the drawing or laying anything out; the drop lays the level out once, as afresh, and stores the position; Escape puts it back")

        # Filters: relation kinds, origins, outside entries, violations only.
        page.locator("#filters-button").click()
        page.locator('#filter-kinds input[data-kind="CALLS"]').uncheck()
        assert any(label.startswith("IMPORTS × 25") for label in page.locator("#graph .edge-label").all_text_contents())
        page.locator("#filter-manual").uncheck()
        assert page.locator("#graph .edge").count() == 1
        page.locator("#filter-outside").uncheck()
        assert page.locator("#graph .node").count() == 2
        page.locator("#filter-violations").check()
        assert page.locator("#graph.violations-only").count() == 1
        assert page.locator("#filters-count").inner_text() == "4"
        page.locator("#filters-reset").click()
        assert page.locator("#graph .node").count() == 3 and page.locator("#graph .edge").count() == 2
        assert page.locator("#filters-count").is_hidden()
        page.keyboard.press("Escape")
        checks.append("filters for relation kinds, manual edges, outside entries and violations only")

        # The node list: the tree with violation badges and a filter.
        assert page.locator("#tree [role='treeitem']").count() == 7
        assert page.locator("#tree [data-id='app.api'] .count-badge").inner_text() == "1"
        page.locator("#tree-violations").check()
        assert page.locator("#tree [role='treeitem']").count() == 3
        page.locator("#tree-violations").uncheck()
        page.locator("#tree [data-id='app.domain'] > .tree-row").click()
        assert page.locator("#details h2").inner_text() == "Domain"
        assert page.locator("#graph .node.selected").get_attribute("aria-label") == "Domain"
        checks.append("sidebar tree with violation badges, violations-only toggle, click selects and centres the entry")

        # Hiding one panel keeps the canvas and the other panel in place.
        def box(selector):
            return page.locator(selector).bounding_box()
        canvas_before, inspector_before = box("#canvas"), box("#inspector")
        page.locator("#sidebar-toggle").click()
        assert page.locator("#sidebar").is_hidden()
        canvas_after, inspector_after = box("#canvas"), box("#inspector")
        assert canvas_after["x"] == 0 and canvas_after["width"] > canvas_before["width"], canvas_after
        assert inspector_after["x"] == inspector_before["x"], inspector_after
        page.locator("#sidebar-toggle").click()
        assert page.locator("#sidebar").is_visible() and box("#canvas") == canvas_before
        page.locator("#inspector-toggle").click()
        assert box("#canvas")["x"] == canvas_before["x"] and box("#canvas")["width"] > canvas_before["width"]
        page.locator("#inspector-toggle").click()
        checks.append("hiding the node list or the inspector widens the canvas and moves nothing else")

        # The table lists the same entries and filters them.
        page.locator("#view-table").click()
        assert page.locator("#stage").is_hidden()
        assert page.locator("#table-wrap .entry-row").count() == 3
        page.locator("#table-filter").fill("dom")
        page.wait_for_function("document.querySelectorAll('#table-wrap .entry-row').length === 1")
        page.locator("#table-wrap .entry-row").click()
        assert "Domain" in page.locator("#details h2").inner_text()
        page.locator("#view-diagram").click()
        assert page.locator("#graph .node").count() == 3
        checks.append("table view with filter and row details, switching back to the canvas")

        # The matrix: rows depend on columns, in the canvas's layer order,
        # counts from the same merged and filtered edges, violations red.
        page.locator("#view-dsm").click()
        assert page.locator("#stage").is_hidden() and page.locator("#dsm-wrap").is_visible()
        rows = page.locator("#dsm-wrap .dsm-row-label .dsm-name").all_text_contents()
        assert rows.index("API") < rows.index("Domain") and len(rows) == 3, rows
        assert page.locator("#dsm-wrap .dsm-group-label").all_text_contents() == ["Application, layer 1 of 2", "Application, layer 2 of 2", "Outside this focus"]
        cell = page.locator("#dsm-wrap .dsm-cell[data-from='node:app.api'][data-to='node:app.domain']")
        assert cell.text_content() == "50" and "CALLS × 25, IMPORTS × 25" in cell.get_attribute("title")
        assert "violating" in cell.get_attribute("class") and cell.evaluate("e => getComputedStyle(e).backgroundColor") == "rgb(196, 34, 27)"
        assert page.locator("#dsm-wrap .dsm-cell").count() == page.evaluate("scene.edges.length") == 2
        manual = page.locator("#dsm-wrap .dsm-cell[data-to='node:external.service']")
        assert "violating" not in manual.get_attribute("class") and manual.text_content() == "1"
        # The diagonal is marked; no entry depends on itself.
        assert page.locator("#dsm-wrap .dsm-diag").count() == 3
        cell.hover()
        assert page.locator("#dsm-wrap .dsm-hl-row").is_visible() and page.locator("#dsm-wrap .dsm-hl-col").is_visible()
        assert page.locator("#dsm-wrap .dsm-row-label.hot .dsm-name").text_content() == "API"
        cell.click()
        assert "CALLS × 25" in page.locator("#details").inner_text() and "selected" in cell.get_attribute("class")
        # Hovering a cell marks its row in the details panel (hover linking).
        page.locator("#dsm-wrap .dsm-row-label", has_text="API").click()
        assert page.locator("#details h2").inner_text() == "API"
        page.locator("#dsm-wrap .dsm-cell[data-to='node:external.service']").hover()
        assert page.locator("#details .dependency.linked").count() == 1
        # The same filters as the canvas.
        page.locator("#filters-button").click()
        page.locator('#filter-kinds input[data-kind="CALLS"]').uncheck()
        assert page.locator("#dsm-wrap .dsm-cell[data-to='node:app.domain']").text_content() == "25"
        page.locator("#filter-manual").uncheck()
        assert page.locator("#dsm-wrap .dsm-cell").count() == 1
        page.locator("#filters-reset").click()
        page.keyboard.press("Escape")
        assert page.locator("#dsm-wrap .dsm-cell").count() == 2
        page.locator("#view-diagram").click()
        assert page.locator("#graph .node").count() == 3
        checks.append("matrix view: rows and columns in layer order with the canvas's groups, counts of merged kinds, violating cell red, diagonal marked, row and column lit on hover, click shows the evidence, same filters as the canvas")

        page.get_by_role("button", name="Domain", exact=True).dblclick()
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")
        assert page.locator("#graph .file").count() == 2
        assert "Domain service" in page.locator("#interfaces").inner_text()
        assert "focus=app.domain" in page.evaluate("window.__history.at(-1)")
        checks.append("double-click focus, leaf files, interfaces, updated deep link")

        # A level too large to draw file by file shows directory groups that
        # expand in place; the table still lists every file.
        page.evaluate("loadFocus('app.big')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.big'")
        assert page.locator("#graph .node.group").count() == 1
        assert page.locator("#graph .node.file").count() == 0
        page.locator("#graph .node.group").dblclick()
        page.wait_for_function("() => scene.entries.filter((n) => n.entry_kind === 'file').length === 200")
        # Zoomed in, only the cards and wires in view (with the overscan) are
        # in the DOM; the rest come back, in drawing order, as the view moves.
        page.locator("#zoom-fit").click()
        page.wait_for_timeout(600)
        order = lambda: page.evaluate("[...document.getElementById('graph').children].map((e) => e.dataset.id || e.dataset.from || e.dataset.trunk || e.getAttribute('class')).join('|')")
        assert page.locator("#graph .node.file").count() == 200
        whole = order()
        page.evaluate("(() => { const b = scene.positions.get(scene.entries.find((n) => n.entry_kind === 'file').id); setCamera({k: 2.5, x: 200 - b.x * 2.5, y: 300 - b.y * 2.5}); })()")
        rendered = page.locator("#graph .node.file").count()
        assert 0 < rendered < 60, rendered
        # An entry out of view still takes the keyboard's focus, and a lifted
        # copy shows it whole.
        far = page.evaluate("scene.entries.filter((n) => n.entry_kind === 'file').at(-1).id")
        assert page.evaluate(f"!scene.nodeEls.get('{far}').isConnected")
        page.evaluate(f"hoverCanvas({{ nodes: new Set(['{far}']), edges: new Set() }}, {{ node: '{far}' }})")
        assert page.locator(f"#lift-graph .node[data-id='{far}']").count() == 1
        page.evaluate("hoverCanvas(null)")
        page.evaluate(f"moveTo('{far}')")
        assert page.evaluate(f"document.activeElement === scene.nodeEls.get('{far}') && scene.nodeEls.get('{far}').isConnected")
        page.locator("#zoom-fit").click()
        page.wait_for_timeout(600)
        assert order() == whole
        assert page.locator("#collapse-groups").is_visible()
        page.locator("#collapse-groups").click()
        assert page.locator("#graph .node.group").count() == 1
        page.locator("#view-table").click()
        assert page.locator("#table-wrap .entry-row").count() == 200
        page.locator("#view-diagram").click()
        checks.append("large levels as expandable directory groups; the table lists every file; zoomed in, only the cards in view are in the DOM, and they come back in drawing order")

        # A matrix of 200 files scrolls in its own pane with sticky headers
        # and never scrolls the page sideways; double-clicking a group row
        # expands it, as on the canvas.
        page.locator("#view-dsm").click()
        assert page.locator("#dsm-wrap .dsm-row-label").count() == 1
        page.locator("#dsm-wrap .dsm-row-label").dblclick()
        page.wait_for_function("() => document.querySelectorAll('#dsm-wrap .dsm-row-label').length === 200")
        scroller = page.locator("#dsm-wrap .dsm")
        assert scroller.evaluate("e => e.scrollWidth > e.clientWidth && e.scrollHeight > e.clientHeight")
        assert page.evaluate("document.documentElement.scrollWidth <= window.innerWidth && document.body.scrollWidth <= window.innerWidth")
        head_top = page.locator("#dsm-wrap .dsm-cols").bounding_box()["y"]
        row_left = page.locator("#dsm-wrap .dsm-rows").bounding_box()["x"]
        scroller.evaluate("e => { e.scrollTop = 2500; e.scrollLeft = 2500; }")
        page.wait_for_timeout(100)
        assert abs(page.locator("#dsm-wrap .dsm-cols").bounding_box()["y"] - head_top) < 1
        assert abs(page.locator("#dsm-wrap .dsm-rows").bounding_box()["x"] - row_left) < 1
        # Long names are cut in the headers and whole in their tooltips.
        assert page.locator("#dsm-wrap .dsm-col-label[title$='src/big/f004.rs']").count() == 1
        page.locator("#collapse-groups").click()
        assert page.locator("#dsm-wrap .dsm-row-label").count() == 1
        page.locator("#view-diagram").click()
        checks.append("matrix of 200 entries: own scroll pane, sticky row and column headers, no sideways page scroll, full names in tooltips, group rows expand and collapse")

        page.locator("#breadcrumbs").get_by_role("link", name="Application", exact=True).click()
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app'")
        page.evaluate("loadFocus('app.domain')")
        page.evaluate("window.dispatchEvent(new PopStateEvent('popstate'))")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app'")
        checks.append("breadcrumb navigation and popstate handler with mocked history")

        # Search sits in a corner of the canvas; entries of this view come first.
        assert page.locator("#canvas #search").count() == 1
        page.locator("#canvas").click(position={"x": 20, "y": 500})
        page.keyboard.press("/")
        assert page.evaluate("document.activeElement.id") == "search"
        page.locator("#search").fill("Domain")
        page.locator("#search-results").get_by_role("button", name="Domain — app.domain", exact=True).wait_for()
        page.keyboard.press("ArrowDown")
        assert page.evaluate("document.activeElement.getAttribute('aria-label')") == "Domain (in this view)"
        page.locator("#search-results").get_by_role("button", name="Domain — app.domain", exact=True).click()
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")
        page.locator(".violation-button").click()
        assert "deny-api-domain" in page.locator("#details").inner_text()
        assert "src/api/a.rs" in page.locator("#details").inner_text()
        assert page.locator("#violations .violation-button.selected").count() == 1
        checks.append("corner search with keyboard (view entries, then architecture nodes), violation evidence selection")

        # Who uses a package: search finds it by name, opens the level of the
        # node owning it and lists every importing file, line and node.
        page.locator("#search").fill("ortool")
        page.locator("#search-results .result-heading", has_text="Packages").wait_for()
        page.locator("#search-results").get_by_role("button", name="ortools — Python package", exact=True).click()
        page.wait_for_function("document.getElementById('focus-id').textContent === 'packages'")
        assert page.locator("#graph .node.package").count() == 2
        assert page.locator("#graph .node.package .package-glyph").count() == 2
        assert page.locator("#graph .node.package.selected").count() == 1
        # The tooltip, not the fitted line, so the check holds with any font width.
        assert "imported by 2 file(s)" in page.locator(f"#graph .node.package[data-id='{ORTOOLS['id']}'] > title").text_content()
        # A long name is cut short of the corner glyph, and the tooltip keeps it whole.
        wide = page.locator(f'#graph .node.package[data-id="{FONTSOURCE["id"]}"]')
        title_right = wide.locator(".node-title").evaluate("e => e.getBBox().x + e.getBBox().width")
        glyph_left = wide.locator(".package-glyph").evaluate("e => e.getBBox().x")
        assert title_right < glyph_left, (title_right, glyph_left, wide.locator(".node-title").text_content())
        assert wide.locator(".node-title").text_content().endswith("…")
        assert "@fontsource/ibm-plex-sans" in wide.locator("title").first.text_content()
        details = page.locator("#details").inner_text()
        for expected in ["Python package", "package:python/ortools", "Imported by 2 files in 2 nodes. Owned by External packages (packages).",
                         "src/domain/a.rs:3", "ortools.constraint_solver", "src/api/a.rs:9", "ortools.sat (type only)", "Used by (2)"]:
            assert expected in details, (expected, details)
        # An importing node drawn in this view is selected in place.
        page.locator("#details .importer-group", has_text="Domain").get_by_role("button", name="Show this node").click()
        assert page.locator("#details h2").inner_text() == "Domain"
        assert page.locator("#graph .node.selected").get_attribute("aria-label") == "Domain"
        assert page.locator("#focus-id").inner_text() == "packages"
        # An edge into packages names each package in large type, with the
        # importing line from this edge's files; the name opens the package.
        page.evaluate("showEdge(scene.edges.find((e) => e.from === 'node:app.domain' && e.to === '%s'))" % ORTOOLS["id"])
        details = page.locator("#details").inner_text()
        assert "Imports of third-party packages" in details and "Observed file dependencies from GitNexus" not in details, details
        assert page.locator("#details .edge-package-name").all_text_contents() == ["ortools"]
        assert page.locator("#details h2").inner_text() == "ortools"
        assert page.locator("#details .edge-package-name").evaluate("e => parseFloat(getComputedStyle(e).fontSize)") >= 18
        package_item = page.locator("#details .edge-package").inner_text()
        assert "Python package" in package_item and "src/domain/a.rs:3" in package_item and "src/api/a.rs:9" not in package_item, package_item
        assert "→ ortools (package)" in details
        page.locator("#details .edge-package-name").click()
        assert page.locator(f"#graph .node.package.selected[data-id='{ORTOOLS['id']}']").count() == 1
        page.evaluate("loadFocus('app.domain')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")
        checks.append("an edge into packages names them in large type with their import lines; package search opens the owner's level with the package selected; its details list importing files, lines and nodes")

        # Usage: a declared entry point is told apart from code nothing
        # observed uses, which is marked calmly, never as a violation.
        page.evaluate("loadFocus('app.web')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.web'")
        assert page.locator("#graph .node.entry-point .usage-mark.entry").count() == 2
        old = page.locator("#graph .node[data-id='node:app.web.old']")
        assert "idle" in old.get_attribute("class") and old.locator(".usage-mark.idle .idle-ring").count() == 1
        assert old.get_attribute("aria-label") == "Old widgets, no observed use from outside, 1 file with no observed users"
        assert page.locator("#graph .node.violating").count() == 0
        ring, red = old.locator(".idle-ring").evaluate("e => [getComputedStyle(e).stroke, getComputedStyle(document.querySelector('.count-badge') || document.body).backgroundColor]")
        assert ring != red and "196, 34, 27" not in ring, ring
        old.click()
        assert "No observed code outside this node depends on it, and it declares no entry point" in page.locator("#details").inner_text()
        page.locator("#graph .node[data-id='node:app.web.ui']").focus()
        page.keyboard.press("Enter")
        details = page.locator("#details").inner_text()
        assert "1 declared entry point · 1 file with no observed users" in details, details
        page.locator("#details").get_by_role("button", name="Show files with no observed users").click()
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.web.ui'")
        assert page.locator("#graph.idle-only").count() == 1
        assert page.locator("#filters-count").inner_text() == "1"
        legacy = page.locator("#graph .node[data-id='file:web/src/legacy.ts']")
        main_tsx = page.locator("#graph .node[data-id='file:web/src/main.tsx']")
        opacity = lambda locator: float(locator.evaluate("e => getComputedStyle(e).opacity"))
        page.wait_for_timeout(300)
        assert opacity(legacy) == 1 and opacity(main_tsx) < 0.5, (opacity(legacy), opacity(main_tsx))
        assert legacy.locator(".node-facts").text_content() == "no observed users"
        assert main_tsx.locator(".node-facts").text_content() == "entry point"
        assert page.locator("#graph .node[data-id='file:web/src/gen.ts'] .node-facts").text_content() == "not indexed"
        assert page.locator("#graph .node[data-id='file:web/src/App.tsx'] .node-facts").count() == 0
        legacy.click()
        details = page.locator("#details").inner_text()
        assert "No observed code depends on this. Possibly an entry point for a tool, or dead code; declare it in project.entry_points if it is an entry point." in details, details
        assert "Not proof" in details
        main_tsx.click()
        assert "Entry point, declared in architecture.yaml" in page.locator("#details").inner_text()
        page.locator("#graph .node[data-id='file:web/src/gen.ts']").click()
        assert "whether anything uses it is unknown" in page.locator("#details").inner_text()
        page.keyboard.press("Escape")
        facts = page.locator("#summary").inner_text()
        assert "Entry points\n1" in facts and "No observed users\n1" in facts, facts
        # The filter is a checkbox like the others; resetting clears it.
        page.locator("#filters-button").click()
        assert page.locator("#filter-idle").is_checked()
        page.locator("#filters-reset").click()
        assert page.locator("#graph.idle-only").count() == 0 and not page.locator("#filter-idle").is_checked()
        page.keyboard.press("Escape")
        # The table shows the usage, filters by it and sorts by it.
        page.locator("#view-table").click()
        assert page.locator("#table-wrap thead").inner_text().split("\t")[1].strip() == "Usage"
        assert page.locator("#table-wrap tr", has_text="legacy.ts").locator(".usage-cell").inner_text() == "no observed users"
        assert page.locator("#table-wrap tr", has_text="main.tsx").locator(".usage-cell").inner_text() == "entry point"
        page.locator("#table-sort").select_option("idle")
        assert "legacy.ts" in page.locator("#table-wrap .entry-row").first.inner_text()
        page.locator("#table-filter").fill("no observed")
        page.wait_for_function("document.querySelectorAll('#table-wrap .entry-row').length === 1")
        page.locator("#table-filter").fill("")
        page.locator("#table-sort").select_option("path")
        page.locator("#view-diagram").click()
        # Search results carry the status too.
        page.locator("#search").fill("legacy")
        page.locator("#search-results").get_by_role("button", name="legacy.ts (in this view), no observed users", exact=True).wait_for()
        assert "web/src/legacy.ts · no observed users" in page.locator("#search-results").inner_text()
        page.keyboard.press("Escape")
        # Dark mode keeps the marks calm and distinct.
        page.emulate_media(color_scheme="dark")
        dark = legacy.locator(".idle-ring").evaluate("e => getComputedStyle(e).stroke")
        assert dark != ring and "255, 116, 104" not in dark, dark
        page.emulate_media(color_scheme="light")
        page.evaluate("loadFocus('app.domain')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")
        checks.append("usage: entry-point and no-observed-users marks on cards (calm, not red, in light and dark), details notes, node-level candidate, filter, table column and sort, search status")

        # Wires: colours, a bus of strands per relation kind, and the two
        # board modes with their angles, tracks and re-routing after a drag.
        page.evaluate("(p) => { window.__fixture.projections['app.wires'] = p; }", wires_projection())
        page.evaluate("loadFocus('app.wires')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.wires'")
        strokes = page.evaluate("() => scene.edgeEls.map((item) => [item.edge.from, item.edge.to, getComputedStyle(item.line).stroke + ' ' + getComputedStyle(item.line).strokeDasharray])")
        # Seven sources on six hues: the seventh repeats a hue with a dash pattern.
        by_source = {}
        for source, _, stroke in strokes:
            by_source.setdefault(source, set()).add(stroke)
        assert all(len(colours) == 1 for colours in by_source.values()), by_source
        firsts = [next(iter(colours)) for colours in by_source.values()]
        assert len(set(firsts)) == len(firsts) == 7, by_source
        assert page.locator("#legend .legend-row").count() == 7
        assert page.locator("#legend .legend-row").first.get_attribute("aria-label").startswith("Wires from ")
        page.locator("#graph .node[data-id='file:w/a.ts']").click()
        assert page.locator("#details .dependency .swatch").count() == 5
        page.keyboard.press("Escape")
        page.locator("#colour-by").select_option("kind")
        strands = page.evaluate("() => scene.edgeEls.filter((item) => item.strands.length).map((item) => [item.edge.from, item.strands.map((s) => getComputedStyle(s).stroke)])")
        assert len(strands) == 1 and strands[0][0] == "file:w/a.ts" and len(set(strands[0][1])) == 2, strands
        page.locator("#colour-by").select_option("target")
        assert page.evaluate("JSON.parse(localStorage.getItem('archgraph.view.v1'))") == {"mode": "curves", "colour": "target", "focus": True}
        page.locator("#colour-by").select_option("source")
        checks.append("wires coloured by source (one colour per entry, all its outgoing wires share it), legend and details swatches, kind colouring with a bus of strands")

        page.locator("#mode-pcb").click()
        assert page.locator("#mode-pcb").get_attribute("aria-pressed") == "true"
        assert page.evaluate("JSON.parse(localStorage.getItem('archgraph.view.v1')).mode") == "pcb"
        assert page.locator("#graph.mode-pcb").count() == 1
        assert not angles_ok(page, 45), angles_ok(page, 45)
        assert not spacing_problems(page, 12), spacing_problems(page, 12)
        # Deterministic: drawing again gives the same traces.
        first = page.evaluate(SEGMENTS)
        page.evaluate("redraw()")
        assert page.evaluate(SEGMENTS) == first
        # Dragging a card one column over drops it on the grid and re-routes.
        card = page.locator("#graph .node[data-id='file:w/d.ts']")
        before = card.get_attribute("transform")
        box = card.bounding_box()
        page.mouse.move(box["x"] + 30, box["y"] + 20)
        page.mouse.down()
        page.mouse.move(box["x"] + 30 + box["width"] * 1.2, box["y"] + 20, steps=8)
        page.mouse.up()
        page.wait_for_timeout(500)
        assert card.get_attribute("transform") != before
        assert page.evaluate(SEGMENTS) != first
        assert not angles_ok(page, 45) and not spacing_problems(page, 12)
        assert page.evaluate("Object.keys(localStorage).some(k => k.startsWith('archgraph.layout-pcb.v1:Browser fixture:app.wires'))")
        page.locator("#reset-layout").click()
        page.wait_for_timeout(500)
        assert page.evaluate(SEGMENTS) == first
        drag_checks(page, ".node[data-id='file:w/d.ts']", "archgraph.layout-pcb.v1:Browser fixture:app.wires", True)
        page.locator("#reset-layout").click()
        page.wait_for_timeout(500)
        assert page.evaluate(SEGMENTS) == first
        # Released over a tool card (the legend) instead of the drawing, a
        # drag still ends: the entry is dropped, not left on the lift layer.
        legend = page.locator("#legend").bounding_box()
        box = card.bounding_box()
        page.mouse.move(box["x"] + 30, box["y"] + 20)
        page.mouse.down()
        page.mouse.move(legend["x"] + legend["width"] / 2, legend["y"] + 12, steps=8)
        page.mouse.up()
        page.wait_for_timeout(500)
        assert page.evaluate("!scene.drag && !document.getElementById('viewport').classList.contains('entry-drag')")
        assert page.locator("#graph .node[data-id='file:w/d.ts']").count() == 1
        assert page.locator("#lift .node").count() == 0
        page.locator("#reset-layout").click()
        page.wait_for_timeout(500)
        assert page.evaluate(SEGMENTS) == first
        checks.append("PCB mode: every segment at a multiple of 45°, parallel traces at least a pitch apart, deterministic routing, a dragged card snaps to the grid and re-routes, positions stored per mode, a drag released over a tool card still drops")

        page.locator("#mode-hex").click()
        assert page.locator("#graph.mode-hex").count() == 1
        assert not angles_ok(page, 60), angles_ok(page, 60)
        assert page.locator("#graph .node .box").first.evaluate("e => e.tagName") == "path"
        checks.append("Hex mode: hexagonal cards, every segment at a multiple of 60°")

        # Zoomed out, titles and wire labels keep a readable size on screen,
        # still fitted inside their cards; far out a card shows only its
        # title and wire labels wait for a hover.
        page.locator("#mode-curves").click()
        text_state = """() => ({
            titles: [...document.querySelectorAll('#graph .node-title')].map((t) => {
                const box = scene.positions.get(t.closest('.node').dataset.id), b = t.getBBox();
                return [parseFloat(getComputedStyle(t).fontSize) * camera.k, b.x + b.width, box.width, t.textContent]; }),
            label: parseFloat(getComputedStyle(document.querySelector('#graph .edge:not(.crowded) .edge-label')).fontSize) * camera.k,
            labelShown: getComputedStyle(document.querySelector('#graph .edge:not(.crowded) .edge-label')).display !== 'none',
            subtitle: getComputedStyle(document.querySelector('#graph .node-subtitle')).display,
            compact: document.getElementById('graph').classList.contains('compact')})"""
        for k in (0.8, 0.65, 0.5, 0.4):
            page.evaluate(f"setCamera({{k: {k}, x: 0, y: 0}})")
            state = page.evaluate(text_state)
            assert all(size >= 11.99 for size, *_ in state["titles"]), (k, state["titles"])
            assert all(right <= width - 8 for _, right, width, _ in state["titles"]), (k, state["titles"])
            assert state["compact"] == (k < 0.6) and (state["subtitle"] == "none") == (k < 0.6), (k, state)
            if k >= 0.6:
                assert state["label"] >= 10.99 and state["labelShown"], (k, state)
            else:
                assert not state["labelShown"], (k, state)
        # A two-line title never strands a character or two on a line of its
        # own: "composables/" one character too wide stays on one line, cut.
        lines = page.evaluate("""() => { const t = document.querySelector('#graph .node-title');
            return ['composables/', 'authentication/', 'test_solver_progress.py'].map((name) => twoLines(t, name, textWidth(t, name.slice(0, -1), 30) + 0.5, 30)); }""")
        assert all(len(line) >= 3 for group in lines for line in group), lines
        assert len(lines[0]) == 1 and lines[0][0].endswith("…") and lines[2] == ["test_solver_", "progress.py"], lines
        # (Clear of the tools over the top of the canvas.)
        page.evaluate("setCamera({k: 0.4, x: 0, y: 200})")
        page.locator("#graph .node[data-id='file:w/a.ts']").hover()
        assert page.locator("#lift-graph .edge.hover .edge-label").first.evaluate("e => getComputedStyle(e).display") != "none"
        page.mouse.move(0, 0)
        page.locator("#zoom-fit").click()
        checks.append("zoomed out, titles and wire labels keep a minimum size on screen and stay inside their cards; far out cards show only their title and labels only on hover")

        # A violation stays red over any wire colour, in every mode.
        page.evaluate("loadFocus('app')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app'")
        for mode in ("curves", "pcb", "hex"):
            page.locator(f"#mode-{mode}").click()
            casing = page.locator("#graph .edge.violating .edge-casing").first.evaluate("e => getComputedStyle(e).stroke")
            core = page.locator("#graph .edge.violating .edge-line").first.evaluate("e => getComputedStyle(e).stroke")
            assert casing == "rgb(196, 34, 27)" and core != casing, (mode, casing, core)
        page.locator("#colour-by").select_option("none")
        assert page.locator("#graph .edge.violating .edge-line").first.evaluate("e => getComputedStyle(e).stroke") == "rgb(196, 34, 27)"
        page.locator("#colour-by").select_option("source")
        page.locator("#mode-curves").click()
        page.evaluate("loadFocus('app.domain')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")
        checks.append("violations keep a red casing over the wire colour in curves, PCB and Hex, and turn red with colouring off")

        # Wires from several siblings into one target merge into a trunk: a
        # ribbon with a strand per source colour, one arrowhead as wide as the
        # ribbon at the target (the wires lose theirs), a tag beside it with
        # the count, chevrons along it, a red casing and a count when one of
        # its wires violates a rule, in every mode.
        page.evaluate("(p) => { window.__fixture.projections['app.trunks'] = p; }", trunk_projection())
        page.evaluate("loadFocus('app.trunks')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.trunks'")
        into_service = """() => [...document.querySelectorAll('#graph .edge:not(.trunk) [marker-end]')].filter((path) =>
            scene.edgeEls.find((item) => item.element === path.closest('.edge')).edge.to === 'node:external.service').length"""
        ribbon = """() => { const trunk = document.querySelector('#graph .edge.trunk[data-trunk]'), service = scene.positions.get('node:external.service');
            const arrow = trunk.querySelector('.trunk-arrow').getBBox(), plate = document.querySelector(`#graph [data-trunk-tag] ${document.getElementById('graph').classList.contains('compact') ? '.tag-short' : '.tag-full'} .tag-plate`).getBoundingClientRect();
            const tip = trunk.querySelector('.trunk-arrow').getBoundingClientRect();
            const strands = [...trunk.querySelectorAll('.trunk-strand')].filter((s) => s.getAttribute('d'));
            return { classes: [...new Set(strands.map((s) => getComputedStyle(s).stroke))].length,
                strandWidth: Math.max(...strands.map((s) => parseFloat(getComputedStyle(s).strokeWidth))),
                arrowWidth: Math.min(arrow.width, arrow.height), arrowAt: [arrow.x + arrow.width / 2, arrow.y + arrow.height, service.x, service.x + service.width, service.y],
                tagGap: Math.max(plate.left - tip.right, tip.left - plate.right, plate.top - tip.bottom, tip.top - plate.bottom),
                markers: trunk.querySelectorAll('[marker-end]').length }; }"""
        for mode in ("curves", "pcb", "hex"):
            page.locator(f"#mode-{mode}").click()
            page.locator("#zoom-fit").click()
            assert page.locator("#graph .edge.trunk[data-trunk]").count() == 1, mode
            assert page.locator("#graph .edge.branch").count() == 6, mode
            assert page.evaluate(into_service) == 0, mode
            tag = page.locator("#graph [data-trunk-tag] .tag-full .tag-text").text_content()
            assert tag.startswith("×6 → Service") and "⚠ 1" in tag, (mode, tag)
            assert page.locator("#graph [data-trunk-tag] .tag-short .tag-text").text_content().startswith("×6"), mode
            trunk = page.locator("#graph .edge.trunk[data-trunk]")
            assert "violating" in trunk.get_attribute("class")
            assert trunk.locator(".trunk-casing").first.evaluate("e => getComputedStyle(e).stroke") == "rgb(196, 34, 27)", mode
            state = page.evaluate(ribbon)
            # Six sources, six colours side by side; the arrowhead as wide as
            # the ribbon, at the target's card, the tag right beside it.
            assert state["classes"] == 6 and "ribbon" in trunk.get_attribute("class"), (mode, state)
            assert state["markers"] == 0 and state["arrowWidth"] >= 6 * 1.8, (mode, state)
            x, bottom, left, right, top = state["arrowAt"]
            assert left <= x <= right and abs(bottom - top) < 30, (mode, state)
            assert state["tagGap"] < 6, (mode, state)
            # Paths that look alike are one path: the casings and hit paths
            # of all six tails, the chevrons, and each colour's strand.
            parts = trunk.evaluate("t => ['.trunk-casing', '.trunk-hit', '.trunk-chevron', '.trunk-strand'].map((s) => t.querySelectorAll(s).length)")
            assert parts == [1, 1, 1, 6], (mode, parts)
            if mode != "curves":
                assert not angles_ok(page, 45 if mode == "pcb" else 60), (mode, angles_ok(page, 45 if mode == "pcb" else 60))
                first = page.evaluate(SEGMENTS)
                page.evaluate("redraw()")
                assert page.evaluate(SEGMENTS) == first, mode
            if mode == "pcb":
                assert not spacing_problems(page, 12), spacing_problems(page, 12)
            # Zoomed in, chevrons point along the trunk at a fixed spacing on
            # screen; zoomed far out, the strands, the wires and their arrowheads
            # keep a minimum width and the count stays readable.
            box = page.evaluate("scene.positions.get('node:external.service')")
            page.evaluate(f"setCamera({{k: 3, x: -({box['x']} + 40) * 3 + 400, y: -({box['y']}) * 3 + 600}})")
            assert page.locator("#graph .edge.trunk .trunk-chevron").count() >= 1, mode
            page.evaluate("setCamera({k: 0.15, x: 300, y: 200})")
            far = page.evaluate("""() => ({
                wire: Math.min(...[...document.querySelectorAll('#graph .edge:not(.trunk) .edge-line')].map((e) => parseFloat(getComputedStyle(e).strokeWidth))) * camera.k,
                strand: Math.min(...[...document.querySelectorAll('#graph .trunk-strand')].map((e) => parseFloat(getComputedStyle(e).strokeWidth))) * camera.k,
                marker: parseFloat(document.getElementById('arrow').getAttribute('markerWidth')) * camera.k,
                arrow: (() => { const b = document.querySelector('#graph .trunk-arrow').getBoundingClientRect(); return Math.max(b.width, b.height); })(),
                count: (() => { const t = document.querySelector('#graph [data-trunk-tag] .tag-short .tag-text'); return getComputedStyle(t).display !== 'none' && getComputedStyle(t.closest('.tag-short')).display !== 'none' ? t.getBoundingClientRect().height : 0; })() })""")
            assert far["wire"] >= 1.45 and far["strand"] >= 1.95 and far["marker"] >= 10.5 and far["arrow"] >= 12 and far["count"] >= 11, (mode, far)
            page.locator("#zoom-fit").click()
            # Hovering the trunk lights its six wires and their entries.
            page.locator("#graph [data-trunk-tag] .trunk-tag").hover()
            assert page.locator("#lift-graph .edge.branch.hover").count() == 6, mode
            assert page.locator("#lift-graph .node.hover").count() == 7, mode
            page.mouse.move(0, 0)
        page.locator("#graph [data-trunk-tag] .trunk-tag").click()
        details = page.locator("#details").inner_text()
        assert "Trunk" in details and "Wires in this trunk (6)" in details and "1 of them violates deny-api-domain" in details, details
        page.locator("#details .dependency", has_text="t/s3.ts").click()
        assert "Observed dependency" in page.locator("#details").inner_text()
        assert page.locator("#graph .edge.branch.selected").count() == 1
        # One wire selected: its own strand shows over the dimmed trunk.
        assert page.locator("#lift-graph .edge.trunk .trunk-overlay .trunk-strand").count() == 1
        # Coloured by kind, one strand per kind (all six are IMPORTS).
        page.locator("#colour-by").select_option("kind")
        assert page.evaluate(ribbon)["classes"] == 1
        # Coloured by target, the trunk is one strand of the target's colour.
        page.locator("#colour-by").select_option("target")
        assert "ribbon" not in page.locator("#graph .edge.trunk[data-trunk]").get_attribute("class")
        assert re.search(r"\bn\d\b", page.locator("#graph .edge.trunk[data-trunk]").get_attribute("class"))
        assert page.evaluate(ribbon)["classes"] == 1
        page.locator("#colour-by").select_option("source")
        page.locator("#mode-curves").click()
        page.evaluate("loadFocus('app.domain')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")
        checks.append("a trunk replaces six parallel wires into one target in curves, PCB and Hex: a ribbon of a strand per source colour (one per kind, one when coloured by target), one arrowhead as wide as the ribbon at the target, the tag beside it, chevrons along it, red casing and a count for a violating wire; zoomed out to 15% wires, strands, arrowheads and the count keep a minimum size; hover lights its wires, details list them, a selected wire's strand shows through")

        # Focus mode: above 60 wires every wire is faint until an entry is
        # pointed at; its own wires light up, violations stay strong, and
        # the toggle is remembered with the other view settings. Below the
        # threshold (the level before) nothing is faint.
        assert page.locator("#graph.faint").count() == 0
        page.evaluate("(p) => { window.__fixture.projections['app.dense'] = p; }", dense_projection())
        page.evaluate("loadFocus('app.dense')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.dense'")
        page.locator("#zoom-fit").click()
        page.wait_for_timeout(500)
        opacity = lambda selector: page.locator(selector).first.evaluate("e => parseFloat(getComputedStyle(e).opacity)")
        # A wire of f05's (not violating), and one f05 has nothing to do with.
        own = "#graph .edge:not(.trunk):not(.violating)[data-from='file:d/f05.ts']"
        other = "#graph .edge:not(.trunk):not(.violating)[data-from='file:d/f01.ts']:not([data-to='file:d/f05.ts'])"
        assert page.evaluate("scene.edges.length") == 66 and page.locator("#graph.faint").count() == 1
        # At rest the other wires are drawn together, a few paths for all of
        # them (one per look: colour and weight), lighter and solid: the wire,
        # not its group, is lightened (so its arrowhead keeps full colour),
        # and a dashed net is drawn solid until it is lit. Each wire keeps its
        # own group with its hit path, so pointing at it still works.
        stroke = lambda selector, prop: page.locator(selector).first.evaluate(f"e => getComputedStyle(e).{prop}")
        batch = page.evaluate("""() => ({ paths: document.querySelectorAll('#graph .edge-batch path').length,
            own: [...document.querySelectorAll('#graph > .edge:not(.trunk):not(.violating)')].map((g) => g.querySelectorAll('.edge-line, .edge-gap').length).reduce((a, b) => a + b, 0),
            batched: document.querySelectorAll('#graph > .edge.batched').length,
            hits: document.querySelectorAll('#graph > .edge.batched > .edge-hit').length,
            arrows: [...document.querySelectorAll('#graph .edge-batch :is(.arrow, .arrow-net)')].map((p) => p.getAttribute('d').split('Z').length - 1).reduce((a, b) => a + b, 0),
            arrowed: document.querySelectorAll('#graph > .edge.batched:not(.branch)').length,
            lines: [...document.querySelectorAll('#graph .edge-batch .edge-line:not(.edge-end)')].map((p) => [parseFloat(getComputedStyle(p).strokeOpacity), getComputedStyle(p).strokeDasharray]),
            groups: [...document.querySelectorAll('#graph .edge-batch > g')].map((g) => parseFloat(getComputedStyle(g).opacity)) })""")
        assert 0 < batch["paths"] <= 40 and batch["own"] == 0 and batch["batched"] > 40 and batch["hits"] == batch["batched"], batch
        assert batch["arrows"] == batch["arrowed"] > 0, batch
        assert all(0.4 <= opacity <= 0.5 and dash == "none" for opacity, dash in batch["lines"]), batch["lines"]
        # The batch's groups keep an opacity below 1, like every wire group.
        assert all(opacity < 1 for opacity in batch["groups"]), batch["groups"]
        assert opacity(own) > 0.99 and opacity(other) > 0.99, (opacity(own), opacity(other))
        dashed = "#graph .edge.c1:not(.trunk):not(.violating)"
        assert page.locator(dashed).count() > 0
        assert opacity("#graph .edge.violating") > 0.99
        # Each arrowhead's wire ends in a solid stretch at full strength: the
        # arrowhead's length and about 12 px more on screen.
        ends = page.evaluate("""() => [...document.querySelectorAll('#graph .edge-line[marker-end]')].map((line) => {
            const end = line.parentNode.querySelector('.edge-end'), style = end && getComputedStyle(end);
            return end ? [parseFloat(style.strokeDasharray) * camera.k, style.strokeOpacity] : null; }).concat(
            [...document.querySelectorAll('#graph .edge-batch .edge-end')].map((end) => [parseFloat(getComputedStyle(end).strokeDasharray) * camera.k, getComputedStyle(end).strokeOpacity]))""")
        assert ends and all(end and end[0] >= 12 and end[1] == "1" for end in ends), ends[:5]
        # Pointing at a wire drawn in the batch lifts a whole copy of it.
        point = page.evaluate("""(selector) => { for (const hit of document.querySelectorAll(selector)) { const length = hit.getTotalLength(), m = hit.getScreenCTM();
            for (let t = 0.2; t < 0.8; t += 0.05) { const q = hit.getPointAtLength(length * t), x = m.a * q.x + m.c * q.y + m.e, y = m.b * q.x + m.d * q.y + m.f;
                const target = document.elementFromPoint(x, y); if (target === hit) return { x, y, from: hit.parentNode.dataset.from, to: hit.parentNode.dataset.to }; } } return null; }""", other + " > .edge-hit")
        assert point, "no visible point on a batched wire"
        page.mouse.move(point["x"], point["y"])
        page.wait_for_timeout(200)
        pointed_wire = f"#lift-graph .edge[data-from='{point['from']}'][data-to='{point['to']}']"
        assert page.locator(pointed_wire + " .edge-line:not(.edge-end)").count() == 1, point
        assert page.locator("#lift-graph .edge.batched").count() == 0
        page.mouse.move(0, 0)
        # Nets past the sixth hue repeat it with dashes, never dots: every
        # drawn piece of a pattern is at least three times the stroke's width.
        patterns = page.evaluate("""() => { const ns = 'http://www.w3.org/2000/svg', box = document.createElementNS(ns, 'svg'); document.body.append(box);
            const out = ['c1', 'c2', 'c3', 'c4'].map((c) => { const g = document.createElementNS(ns, 'g'), line = document.createElementNS(ns, 'path'), strand = document.createElementNS(ns, 'path');
                g.setAttribute('class', `edge w1 n0 ${c}`); line.setAttribute('class', 'edge-line'); strand.setAttribute('class', `trunk-strand ${c}`); g.append(line, strand); box.append(g);
                const dashes = (e) => getComputedStyle(e).strokeDasharray.split(/[ ,]+/).map(parseFloat).filter((v, i) => i % 2 === 0);
                return [c, dashes(line), parseFloat(getComputedStyle(line).strokeWidth), dashes(strand), parseFloat(getComputedStyle(strand).strokeWidth)]; });
            box.remove(); return out; }""")
        assert all(min(line) >= 3 * lw and min(strand) >= 2 * sw for _, line, lw, strand, sw in patterns), patterns
        assert len({tuple(line) for _, line, *_ in patterns}) == 4, patterns
        # Arrowheads are cased in the sheet's colour, so they read on the grid.
        casing = page.evaluate("() => { const a = getComputedStyle(document.querySelector('#arrow-n0 path')); return [a.stroke, parseFloat(a.strokeWidth), a.paintOrder, getComputedStyle(document.getElementById('stage')).getPropertyValue('--sheet').trim()]; }")
        assert casing[1] >= 1 and casing[2].startswith("stroke"), casing
        if page.locator("#graph .edge.trunk[data-trunk]").count():
            assert opacity("#graph .edge.trunk[data-trunk]") > 0.99 and opacity("#graph .edge.trunk .trunk-body") == 1
        page.locator("#graph .node[data-id='file:d/f05.ts']").hover()
        page.wait_for_timeout(300)
        lifted = lambda selector: selector.replace("#graph", "#lift-graph")
        assert opacity(lifted(own)) == 1 and page.locator(lifted(other)).count() == 0, opacity(lifted(own))
        assert stroke(lifted(own) + " .edge-line", "strokeOpacity") == "1"
        assert page.locator("#lift-graph .edge-end[marker-end]").count() == 0
        page.locator("#graph .node[data-id='file:d/f07.ts']").hover()
        lit_dash = page.locator(lifted(dashed) + " .edge-line:not(.edge-end)").first.evaluate("e => [getComputedStyle(e).strokeDasharray.split(/[ ,]+/).map(parseFloat), parseFloat(getComputedStyle(e).strokeWidth)]")
        # Lit, a dashed net shows its pattern, in dashes as long as at rest
        # relative to the heavier stroke.
        assert min(lit_dash[0][::2]) >= 3 * lit_dash[1], lit_dash
        assert opacity("#stage") < 0.3
        page.mouse.move(0, 0)
        page.wait_for_timeout(300)
        assert opacity("#stage") == 1
        page.locator("#focus-mode").click()
        page.wait_for_timeout(300)
        assert page.locator("#graph.faint").count() == 0 and opacity(other) > 0.99
        # Not faint, every wire is drawn on its own again.
        assert page.locator("#graph .edge-batch path").count() == 0 and page.locator(other + " .edge-line").count() > 0
        assert page.locator("#focus-mode").get_attribute("aria-pressed") == "false"
        assert page.evaluate("JSON.parse(localStorage.getItem('archgraph.view.v1')).focus") is False
        page.locator("#focus-mode").click()
        assert page.locator("#graph.faint").count() == 1
        checks.append("focus mode: above 60 wires the others are dimmed to .35 while trunks and violations stay at full strength, an entry's own wires light up on hover; the toggle is remembered")

        # Hovering and zooming stay cheap on a large level: a hover changes
        # nothing in the drawing itself (no class or attribute on any of its
        # elements, so nothing is restyled or repainted there), and neither
        # routes nor lays trunks out; a wheel gesture only moves the stage
        # layer until it pauses.
        page.evaluate("""() => { window.__calls = { route: 0, layoutTrunk: 0, drawScene: 0 };
            for (const name of Object.keys(window.__calls)) { const f = window[name]; window[name] = function (...a) { window.__calls[name]++; return f.apply(this, a); }; }
            window.__mutations = 0; new MutationObserver((list) => { window.__mutations += list.length; }).observe(document.getElementById('graph'), { subtree: true, attributes: true, childList: true }); }""")
        for node_id in ("file:d/f02.ts", "file:d/f05.ts", "file:d/f09.ts"):
            page.locator(f"#graph .node[data-id='{node_id}']").hover()
            page.locator(f"#graph .edge[data-from='{node_id}']").first.hover(force=True)
        page.mouse.move(0, 0)
        page.wait_for_timeout(100)
        assert page.evaluate("window.__mutations") == 0 and page.evaluate("window.__calls") == {"route": 0, "layoutTrunk": 0, "drawScene": 0}, (page.evaluate("window.__mutations"), page.evaluate("window.__calls"))
        # Every wire group carries an opacity of its own (below 1): without
        # one, each frame's layerization is 30 times slower on a large level.
        assert page.evaluate("[...document.querySelectorAll('#graph .edge')].every((e) => parseFloat(getComputedStyle(e).opacity) < 1)")
        stage = page.locator("#canvas").bounding_box()
        page.mouse.move(stage["x"] + stage["width"] / 2, stage["y"] + stage["height"] / 2)
        for _ in range(2):
            page.mouse.wheel(0, -120)
        assert page.evaluate("window.__mutations") == 0 and page.evaluate("window.__calls")["layoutTrunk"] == 0, (page.evaluate("window.__mutations"), page.evaluate("window.__calls"))
        page.wait_for_timeout(400)
        assert page.evaluate("window.__mutations") > 0
        checks.append("a hover changes nothing in the drawing and neither routes nor lays out trunks; a wheel gesture only moves the stage layer until it pauses")
        page.evaluate("loadFocus('app.domain')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")

        # A live server publishes a new revision: the view follows it and stays
        # on the current node; a failed reload is shown, not hidden.
        assert page.locator("#snapshot").inner_text() == "Read-only snapshot"
        page.evaluate("""() => {
            const fixture = window.__fixture;
            fixture.meta = {...fixture.meta, watching: true, revision: 2, diagnostics: ['Reloaded coverage warning']};
            fixture.projections['app.domain'].focus.title = 'Domain v2';
        }""")
        assert page.evaluate("checkForUpdates()") is True
        assert page.locator("#focus-title").inner_text() == "Domain v2"
        assert page.locator("#snapshot").inner_text() == "Live at revision 2"
        assert "Reloaded coverage warning" in page.locator("#diagnostics").text_content()
        assert page.locator("#notes-button").inner_text() == "1 coverage note"
        assert page.locator("#refresh-status").is_hidden()
        page.evaluate("() => { window.__fixture.meta = {...window.__fixture.meta, refresh_error: 'index changed while compiling'}; }")
        assert page.evaluate("checkForUpdates()") is False
        assert "latest reload failed: index changed while compiling" in page.locator("#refresh-status").inner_text()
        page.evaluate("""() => {
            const fixture = window.__fixture;
            fixture.meta = {...fixture.meta, revision: 3, refresh_error: null};
            delete fixture.projections['app.domain'];
        }""")
        assert page.evaluate("checkForUpdates()") is True
        assert page.locator("#focus-title").inner_text() == "Application"
        assert page.locator("#refresh-status").is_hidden()
        assert page.locator("#error").is_hidden()
        checks.append("live reload follows new revisions, keeps the focus while it exists, and reports failed reloads")

        # The code viewer: selecting a file shows its source in the second
        # pane, an evidence line scrolls to the lines it points at, and a
        # path the viewer must not ask for is refused before any fetch.
        page.evaluate("loadFocus('app.web.ui')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.web.ui'")
        assert page.locator("#pane-secondary").is_hidden()
        app_card = "#graph .node[data-id='file:web/src/App.tsx']"
        before = canvas_place(page, app_card)
        page.locator(app_card).click()
        page.wait_for_selector("#pane-secondary[data-state='shown']")
        # The inspector widens and the canvas narrows; the selected card
        # keeps its place in the visible canvas instead of staying put
        # against the canvas's left edge and drifting off centre.
        page.wait_for_timeout(50)
        after = canvas_place(page, app_card)
        assert after["width"] < before["width"] - 100, (before, after)
        assert abs(after["fx"] - before["fx"]) * after["width"] < 2 and abs(after["fy"] - before["fy"]) * after["height"] < 2, (before, after)
        assert page.locator("body.viewer-open").count() == 1 and page.locator(".pane-splitter").is_visible()
        assert page.locator("#pane-secondary .code-row").count() == 5
        assert page.locator("#pane-secondary .code-text").first.inner_text() == "import { main } from './main';"
        assert page.locator("#viewer-title").text_content() == "web/src/App.tsx"
        assert page.locator("#pane-secondary .code-ln").nth(3).inner_text() == "4"
        # Light colouring: keywords, strings and comments.
        assert page.locator("#pane-secondary .t-keyword", has_text="export").count() == 1
        assert page.locator("#pane-secondary .t-string", has_text='"app"').count() == 1
        assert page.locator("#pane-secondary .t-comment").inner_text() == "// a comment"
        checks.append("selecting a file card opens its source with line numbers and light syntax colouring in the split right panel")
        # Closing the viewer gives the width back, the card still in place.
        page.locator("#viewer-close").click()
        page.wait_for_timeout(50)
        closed = canvas_place(page, app_card)
        assert abs(closed["width"] - before["width"]) < 1 and abs(closed["fx"] - before["fx"]) * closed["width"] < 2, (before, closed)
        # Without a selection the view's centre stays the centre, here when
        # the node list is hidden and shown again.
        page.evaluate("showOverview()")
        centre = canvas_centre(page)
        for _ in range(2):
            page.locator("#sidebar-toggle").click()
            page.wait_for_timeout(50)
            moved = canvas_centre(page)
            assert abs(moved[0] - centre[0]) < 2 and abs(moved[1] - centre[1]) < 2, (centre, moved)
        checks.append("the canvas keeps the selected card, or the view's centre, in place when the viewer opens or closes or a panel is shown or hidden")
        page.locator(app_card).click()
        page.wait_for_selector("#pane-secondary[data-state='shown']")

        # The divider is a separator that remembers its position.
        page.locator(".pane-splitter").focus()
        page.keyboard.press("ArrowDown")
        assert page.locator(".pane-splitter").get_attribute("aria-valuenow") == "45"
        assert page.evaluate("JSON.parse(localStorage.getItem('archgraph.viewer.v1')).split") == 0.45

        # Live reload: a new revision reloads the open file; between
        # revisions a changed file is only marked stale, with a reload button.
        page.evaluate("""() => { const f = window.__fixture; f.meta = {...f.meta, revision: f.meta.revision + 1};
            f.sources['web/src/App.tsx'] = "import { main } from './main';\\n// reloaded\\n"; }""")
        assert page.evaluate("checkForUpdates()") is True
        page.wait_for_function("document.querySelectorAll('#pane-secondary .code-row').length === 2")
        assert "file changed" in page.locator(".viewer-stale").inner_text() and page.locator("#pane-secondary[data-stale]").count() == 0
        page.evaluate("() => { window.__fixture.sources['web/src/App.tsx'] = 'changed on disk\\n'; }")
        assert page.evaluate("checkForUpdates()") is False
        assert "changed on disk" in page.locator(".viewer-stale").inner_text() and page.locator("#pane-secondary[data-stale='true']").count() == 1
        page.locator(".viewer-stale button", has_text="Reload").click()
        page.wait_for_function("document.querySelector('#pane-secondary .code-text').textContent === 'changed on disk'")
        assert page.locator(".viewer-stale").is_hidden()
        checks.append("live reload refreshes an open file with a new revision, and marks it stale with a Reload button when only the file changed")

        # An evidence line: the source file opens scrolled to the line that
        # imports the target (found in the text: GitNexus gives no lines).
        page.evaluate("loadFocus('app')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app'")
        page.locator("#graph .edge-label", has_text="2 kinds").click()
        page.locator("#details .evidence .source-link", has_text="src/api/a.rs").first.click()
        page.wait_for_selector("#pane-secondary[data-path='src/api/a.rs']")
        page.wait_for_timeout(100)
        current = page.locator("#pane-secondary .code-row.current")
        assert current.get_attribute("data-line") == "150" and "guess" in current.get_attribute("class")
        scroller, row = page.locator(".code-scroll").bounding_box(), current.bounding_box()
        assert scroller["y"] <= row["y"] and row["y"] + row["height"] <= scroller["y"] + scroller["height"], (scroller, row)
        assert "Lines that import src/domain/a" in page.locator(".viewer-marks").inner_text()
        # A package import comes with its line from the package report.
        page.evaluate("loadFocus('packages')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'packages'")
        page.locator("#graph .node[data-id='package:python/ortools']").click()
        page.locator("#details .source-link", has_text="src/domain/a.rs:3").click()
        page.wait_for_selector("#pane-secondary[data-path='src/domain/a.rs']")
        assert page.evaluate("viewer.current().marks") == [3] and page.evaluate("viewer.current().exact") is True
        assert page.locator("#pane-secondary .code-row.current.exact").get_attribute("data-line") == "3"
        checks.append("an evidence line opens its file scrolled to and highlighting the importing line (text search, marked as such) or the package report's exact line")

        # Hovering the canvas with the viewer open still changes nothing in the drawing.
        page.evaluate("loadFocus('app')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app'")
        page.locator("#graph .edge-label", has_text="2 kinds").click()
        page.locator("#details .evidence .source-link", has_text="src/api/a.rs").first.click()
        page.wait_for_selector("#pane-secondary[data-path='src/api/a.rs']")
        page.wait_for_timeout(600)
        page.evaluate("""() => { window.__viewerMutations = 0; new MutationObserver((list) => { window.__viewerMutations += list.length; }).observe(document.getElementById('graph'), { subtree: true, attributes: true, childList: true }); }""")
        for name in ("API", "Domain", "API"):
            page.get_by_role("button", name=name, exact=True).hover()
            page.wait_for_timeout(50)
        page.mouse.move(0, 0)
        page.wait_for_timeout(100)
        assert page.evaluate("window.__viewerMutations") == 0, page.evaluate("window.__viewerMutations")
        checks.append("with the viewer open, a hover still makes no mutation in #graph")

        # Refused paths: an unsafe path is refused without a request, and
        # the server's refusal of an unmapped file is shown as it says.
        fetches = page.evaluate("window.__sourceFetches")
        for path in ("../etc/passwd", "/etc/passwd", "src/./api/a.rs", "package:python/ortools"):
            assert page.evaluate("(p) => openSource({ path: p })", path) is False
            assert page.locator("#pane-secondary[data-state='refused']").count() == 1
            assert "Cannot show this file" in page.locator(".viewer-refusal").inner_text()
        assert page.evaluate("window.__sourceFetches") == fetches
        assert page.evaluate("openSource({ path: 'notes.txt' })") is False
        assert page.evaluate("window.__sourceFetches") == fetches + 1
        assert "not a mapped file" in page.locator(".viewer-refusal").inner_text()
        assert page.locator(".code-scroll").is_hidden()
        checks.append("unsafe paths are refused before any fetch; the endpoint's refusal of an unmapped file is shown")

        # Markup colouring: a Vue component's template, script and style, and
        # ERB's Ruby holes, with states carried over lines.
        def spans(line):
            return page.evaluate("""(n) => [...document.querySelectorAll(`#pane-secondary .code-row[data-line='${n}'] .code-text > span`)]
                .map((s) => `${s.className.slice(2)}:${s.textContent}`)""", line)
        assert page.evaluate("openSource({ path: 'web/src/Card.vue' })") is True
        assert spans(2) == ["keyword:<div", 'string:"card"', 'string:"label"'], spans(2)
        assert spans(4) == ["comment:         over two lines -->"], spans(4)
        assert spans(5) == ["keyword:<span", "keyword:</span"], spans(5)
        assert spans(10) == ["keyword:import", "keyword:from", 'string:"vue"'], spans(10)
        assert spans(11) == ["keyword:const", 'string:"card"', "comment:// shown"], spans(11)
        assert spans(12) == ["keyword:</script"], spans(12)
        assert spans(15) == ["number:#333"], spans(15)
        assert page.evaluate("openSource({ path: 'app/views/cards/show.html.erb' })") is True
        assert spans(1) == ["keyword:<h1", 'string:"title"', "type:<%=", "type:%>", "keyword:</h1"], spans(1)
        assert spans(2) == ["type:<%", "keyword:if", "type:%>"], spans(2)
        assert spans(5) == ["type:<%#", "comment: a note for the template ", "type:%>"], spans(5)
        assert spans(6) == ["keyword:<a", 'string:"', "type:<%=", "type:%>", 'string:"', "keyword:</a"], spans(6)
        assert spans(8) == ["comment:# in cents"], spans(8)
        assert spans(9) == ["type:%>"], spans(9)
        checks.append("Vue components colour their template tags, TypeScript and CSS; ERB colours Ruby inside <% %> (also in attribute values and over lines) and tags outside")

        # A large file: only the rows in view are elements, and it opens and
        # scrolls quickly.
        timing = page.evaluate("""async () => {
            const started = performance.now();
            await openSource({ path: 'src/big/huge.ts' });
            const opened = performance.now() - started;
            const scroller = document.querySelector('.code-scroll');
            const times = [];
            for (const line of [5000, 10000, 15000, 19990]) {
                const t = performance.now();
                scroller.scrollTop = (line - 1) * 20;
                scroller.dispatchEvent(new Event('scroll'));
                await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
                times.push(performance.now() - t);
            }
            return { opened, times, prepare: viewer.current().prepareMs, rows: document.querySelectorAll('#pane-secondary .code-row').length,
                     last: document.querySelector('#pane-secondary .code-row:last-child').dataset.line };
        }""")
        assert timing["rows"] < 300 and int(timing["last"]) == 20000, timing
        assert timing["opened"] < 1500 and max(timing["times"]) < 250, timing
        checks.append(f"a 20,000-line file renders only {timing['rows']} rows: opened in {timing['opened']:.0f} ms (scan {timing['prepare']:.0f} ms), each jump while scrolling within {max(timing['times']):.0f} ms")

        page.locator("#viewer-close").click()
        assert page.locator("#pane-secondary").is_hidden() and page.locator("body.viewer-open").count() == 0
        page.locator("#graph .edge-label", has_text="2 kinds").click()
        page.locator("#details .evidence .source-link", has_text="src/api/a.rs").first.click()
        page.wait_for_selector("#pane-secondary[data-path='src/api/a.rs']")
        page.keyboard.press("Escape")
        assert page.locator("#pane-secondary").is_hidden()
        checks.append("the viewer closes with its button, and with the selection on Escape")

        # The inspector's left edge resizes the whole right panel, by pointer
        # and by keyboard. Details alone and details with the source have
        # their own remembered widths, and the canvas keeps 320 px.
        def width(selector):
            return page.locator(selector).bounding_box()["width"]
        handle = page.locator("#inspector-resizer")
        remembered = lambda: page.evaluate("JSON.parse(localStorage.getItem('archgraph.inspector.v1') || '{}')")
        start = width("#inspector")
        edge = handle.bounding_box()
        page.mouse.move(edge["x"] + edge["width"] / 2, edge["y"] + edge["height"] / 3)
        page.mouse.down()
        page.mouse.move(edge["x"] + edge["width"] / 2 - 120, edge["y"] + edge["height"] / 3, steps=6)
        # While dragged only the edge moves; the width is applied on release.
        assert width("#inspector") == start and page.locator("#inspector-resizer.dragging").count() == 1
        assert abs(handle.bounding_box()["x"] - (edge["x"] - 120)) < 1
        page.mouse.up()
        assert abs(width("#inspector") - (start + 120)) < 1.5, (start, width("#inspector"))
        assert remembered() == {"details": round(start + 120)}, remembered()
        assert handle.get_attribute("aria-valuenow") == str(round(width("#inspector")))
        handle.focus()
        page.keyboard.press("ArrowLeft")
        assert abs(width("#inspector") - (start + 144)) < 1.5
        page.keyboard.press("ArrowRight")
        page.keyboard.press("ArrowRight")
        assert abs(width("#inspector") - (start + 96)) < 1.5
        page.keyboard.press("End")
        assert abs(width("#canvas") - 320) < 1.5, width("#canvas")
        page.keyboard.press("Home")
        assert abs(width("#inspector") - 300) < 1.5 and remembered() == {"details": 300}
        # Escape during a drag puts the edge back.
        edge = handle.bounding_box()
        page.mouse.move(edge["x"] + 5, edge["y"] + 200)
        page.mouse.down()
        page.mouse.move(edge["x"] - 200, edge["y"] + 200, steps=4)
        page.keyboard.press("Escape")
        page.mouse.up()
        assert abs(width("#inspector") - 300) < 1.5 and page.locator("#inspector-resizer.dragging").count() == 0
        # With the source open the panel starts wide, and its width is its own.
        page.locator("#graph .edge-label", has_text="2 kinds").click()
        page.locator("#details .evidence .source-link", has_text="src/api/a.rs").first.click()
        page.wait_for_selector("#pane-secondary[data-path='src/api/a.rs']")
        wide = width("#inspector")
        assert wide > 500, wide
        handle.focus()
        page.keyboard.press("Shift+ArrowRight")
        assert abs(width("#inspector") - (wide - 96)) < 1.5
        assert remembered() == {"details": 300, "source": round(wide - 96)}, remembered()
        page.locator("#viewer-close").click()
        assert abs(width("#inspector") - 300) < 1.5
        # A double-click gives the default width back; the canvas follows.
        handle.dblclick()
        assert abs(width("#inspector") - 360) < 1.5 and remembered() == {"source": round(wide - 96)}
        checks.append("the right panel resizes from its left edge by drag (applied on release, Escape cancels) and by arrow keys, Home, End; details and details with source keep their own remembered widths; the canvas keeps 320 px; double-click resets")

        # Phone width: the panel is a drawer, with no resize edge, and nothing
        # scrolls the page sideways, with the source open or not.
        page.set_viewport_size({"width": 390, "height": 844})
        page.wait_for_timeout(100)
        assert handle.is_hidden()
        assert page.evaluate("document.documentElement.scrollWidth") == 390
        assert page.evaluate("openSource({ path: 'src/api/a.rs' })") is True
        page.wait_for_timeout(300)
        assert page.locator("#inspector").bounding_box()["width"] == 390
        assert page.evaluate("document.documentElement.scrollWidth") == 390
        page.evaluate("viewer.close(); togglePanel('inspector', false)")
        page.set_viewport_size({"width": 1440, "height": 1000})
        page.wait_for_timeout(100)
        checks.append("at phone width the right panel is a drawer without a resize edge and the page never scrolls sideways")

        # Comparing with a snapshot: the Compare control lists the saved
        # snapshots; choosing one draws the level's changes as revisions.
        def click(selector):
            page.evaluate("(s) => document.querySelector(s).dispatchEvent(new MouseEvent('click', { bubbles: true }))", selector)
        def tag(selector):
            return page.locator(selector + " > .rev-tag .rev-text").text_content()
        page.evaluate("loadFocus('app')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app'")
        page.locator("#compare-button").click()
        page.wait_for_selector("#compare-list input[value='before']")
        assert "commit 5b29b17" in page.locator("#compare-list").inner_text()
        assert page.locator("#compare-list input[value='']").is_checked() and page.locator("#compare-only").is_disabled()
        page.locator("#compare-list input[value='before']").check()
        page.wait_for_selector("#graph .ghost-card")
        assert "compare=before" in page.evaluate("location.search")
        assert json.loads(page.evaluate("localStorage.getItem('archgraph.compare.v1:Browser fixture')"))["name"] == "before"
        assert page.locator("#compare-label").inner_text() == "Since before" and page.locator("#compare-count").inner_text() == "8"
        page.keyboard.press("Escape")
        assert page.locator("#compare-panel").is_hidden()
        # Entries: new, and another file count; each tag has a glyph, not only a colour.
        api_card, service_card = "#graph .node[data-id='node:app.api']", "#graph .node[data-id='node:external.service']"
        assert page.locator(api_card + ".rev-resized").count() == 1 and tag(api_card) == "▲ 1→2 files"
        assert page.locator(service_card + ".rev-new").count() == 1 and tag(service_card) == "+ new"
        assert page.locator("#graph .node[data-id='node:app.domain'].rev").count() == 0
        # Wires: a new one between rails of the revision ink, one whose count
        # changed (IMPORTS 10 → 25 and CALLS new: 10 → 50 in all).
        changed_wire = "#graph .edge[data-from='node:app.api'][data-to='node:app.domain']"
        new_wire = "#graph .edge[data-from='node:app.api'][data-to='node:external.service']"
        assert page.locator(changed_wire + ".rev-changed").count() == 1 and tag(changed_wire) == "▲ 10→50"
        assert page.locator(new_wire + ".rev-new .rev-casing").count() == 1 and tag(new_wire) == "+ new"
        # Gone: a ghost card for Legacy in a band below the level, and phantom
        # lines (dashed, open arrowhead) for the gone wires, also into the ghost.
        ghost = page.locator("#graph .ghost-card[data-ghost='node:app.legacy']")
        assert ghost.count() == 1 and "gone since before" in ghost.text_content() and tag("#graph .ghost-card") == "− gone"
        ghost_y = page.evaluate("scene.ghostBoxes.get('node:app.legacy').y")
        assert all(ghost_y > box["y"] + box["height"] for box in page.evaluate("[...scene.positions.values()]"))
        ghosts = page.evaluate("[...document.querySelectorAll('#graph .edge.ghost')].map((g) => [g.dataset.from, g.dataset.to, g.querySelector('.ghost-line').getAttribute('marker-end'), getComputedStyle(g.querySelector('.ghost-line')).strokeDasharray, g.querySelector('.rev-text').textContent])")
        assert sorted(g[:2] for g in ghosts) == [["node:app.domain", "node:app.api"], ["node:app.legacy", "node:app.domain"]], ghosts
        assert all(g[2] == "url(#arrow-ghost)" and g[3] != "none" for g in ghosts) and sorted(g[4] for g in ghosts) == ["− 3", "− 7"], ghosts
        # The legend explains the marks present.
        legend = page.locator("#legend-body").inner_text()
        for text in ("Since snapshot before", "new entry", "file count changed", "new wire", "wire with another count", "gone: phantom line"):
            assert text in legend, legend
        # The level's details: counts, violations that appeared (opening
        # their evidence) and were resolved, what is gone, and the files.
        section = page.locator("#compare-section").inner_text()
        for text in ("Since snapshot before", "commit 5b29b17", "1 new", "1 resized", "1 gone", "2 new", "1 changed count", "2 gone", "1 appeared", "1 resolved",
                     "legacy-is-frozen", "app.legacy", "src/domain/old_a.rs", "src/legacy/x.rs"):
            assert text in section, (text, section)
        assert page.locator("#compare-section .overview-violation.resolved").is_disabled()
        page.locator("#compare-section .overview-violation:not(.resolved)").click()
        assert page.locator("#details .detail-kind").inner_text() == "Violation: deny dependency"
        # A selected entry, wire or ghost says how it changed.
        click(api_card)
        assert "1 mapped file in the snapshot, 2 now." in page.locator("#details .rev-note").inner_text()
        click(changed_wire)
        note = page.locator("#details .rev-note").inner_text()
        assert "10 observations in the snapshot, 50 now." in note and "IMPORTS: 10 → 25" in note and "CALLS: new" in note, note
        click("#graph .edge.ghost[data-from='node:app.domain']")
        assert page.locator("#details .detail-kind").inner_text() == "Gone since snapshot before" and "CALLS × 3" in page.locator("#details").inner_text()
        assert page.locator("#lift-graph .edge.ghost").count() == 1
        click("#graph .ghost-card")
        assert page.locator("#details h2").inner_text() == "Legacy" and "→ app.domain" in page.locator("#details").inner_text()
        assert page.locator("#lift-graph .ghost-card").count() == 1 and page.locator("#lift-graph .node[data-id='node:app.domain']").count() == 1
        page.keyboard.press("Escape")
        # Only changes: everything unchanged is dimmed, changes are not.
        page.locator("#compare-button").click()
        page.locator("#compare-only").check()
        page.keyboard.press("Escape")
        opacity = lambda selector: float(page.evaluate("(s) => getComputedStyle(document.querySelector(s)).opacity", selector))
        assert page.locator("#graph.changes-only").count() == 1
        assert opacity("#graph .node[data-id='node:app.domain']") < 0.2 and opacity(api_card) == 1 and opacity("#graph .ghost-card") == 1
        assert opacity(changed_wire) > 0.9
        page.evaluate("setCompareOnly(false)")
        assert page.locator("#graph.changes-only").count() == 0 and opacity("#graph .node[data-id='node:app.domain']") > 0.9
        # On a board the gone wires take one elbow or a Z.
        page.evaluate("setDisplay({ mode: 'pcb' })")
        assert page.locator("#graph .edge.ghost").count() == 2
        assert all(set(re.sub(r"[^A-Z]", "", d)) <= {"M", "L"} for d in page.evaluate("[...document.querySelectorAll('#graph .ghost-line')].map((p) => p.getAttribute('d'))"))
        page.evaluate("setDisplay({ mode: 'curves' })")
        checks.append("compare with a snapshot: the control lists snapshots and is remembered and linked (?compare=); new, moved and resized entries carry glyph tags, new wires rails and a tag, changed counts ▲/▼ tags, gone wires phantom lines and gone entries hatched ghost cards below the level; a legend, the level's changes with violations appeared (opening their evidence) and resolved, the selection's change, and an only-changes filter")

        # A renamed file is one entry, moved; a level the snapshot did not
        # have is all new, and says so instead of marking every entry.
        page.evaluate("(p) => { window.__fixture.projections['app.domain'] = p; }", PROJECTIONS["app.domain"])
        page.evaluate("loadFocus('app.domain')")
        page.wait_for_selector("#graph .node[data-id='file:src/domain/a.rs'].rev-moved")
        assert tag("#graph .node[data-id='file:src/domain/a.rs']") == "↦ moved" and tag("#graph .node[data-id='file:src/domain/b.rs']") == "+ new"
        click("#graph .node[data-id='file:src/domain/a.rs']")
        assert "Moved since the snapshot, from src/domain/old_a.rs." in page.locator("#details .rev-note").inner_text()
        page.keyboard.press("Escape")
        page.evaluate("loadFocus('app.web')")
        page.wait_for_function("document.getElementById('compare-count').textContent === 'new'")
        assert page.locator("#graph .rev").count() == 0
        assert "everything on it is new" in page.locator("#legend-body").inner_text() and "everything on it is new" in page.locator("#compare-section").inner_text()
        # A snapshot that cannot be read is an error, never a clean comparison.
        page.evaluate("setCompare({ on: true, name: 'deleted' })")
        assert page.locator("#compare-count").inner_text() == "!"
        assert "Cannot compare with deleted: no snapshot named `deleted`" in page.locator("#compare-section").inner_text()
        assert page.locator("#graph .rev, #legend .legend-subhead").count() == 0
        # No snapshots yet: the control says how to take one.
        page.evaluate("() => { window.__fixture.snapshots = { enabled: true, snapshots: [] }; }")
        page.locator("#compare-button").click()
        page.wait_for_function("document.getElementById('compare-note').textContent.includes('No snapshot is saved yet')")
        assert "archgraph snapshot" in page.locator("#compare-note").inner_text()
        page.keyboard.press("Escape")
        page.evaluate("() => { window.__fixture.snapshots = { enabled: false, snapshots: [] }; }")
        page.locator("#compare-button").click()
        page.wait_for_function("document.getElementById('compare-note').textContent.includes('no snapshots to compare with')")
        page.keyboard.press("Escape")
        page.evaluate("(s) => { window.__fixture.snapshots = s; }", SNAPSHOTS)
        # Off again: nothing extra drawn, the address forgets it; the address
        # alone turns it on (a deep link).
        page.evaluate("loadFocus('app')")
        page.locator("#compare-button").click()
        page.wait_for_selector("#compare-list input[value='before']")
        page.locator("#compare-list input[value='']").check()
        page.keyboard.press("Escape")
        page.wait_for_function("document.getElementById('compare-label').textContent === 'Compare'")
        assert page.locator("#graph .rev, #graph .ghost-card, #graph .edge.ghost, #compare-section").count() == 0
        assert "compare" not in page.evaluate("location.search")
        assert page.evaluate("history.replaceState(null, '', '?focus=app&compare=before'); loadCompareSettings(); [compare.on, compare.name]") == [True, "before"]
        page.evaluate("setCompare({ on: false })")
        # At phone width the control and its panel fit.
        page.set_viewport_size({"width": 390, "height": 844})
        page.locator("#compare-button").click()
        page.wait_for_selector("#compare-list input[value='before']")
        assert page.locator("#compare-panel").bounding_box()["x"] + page.locator("#compare-panel").bounding_box()["width"] <= 390
        assert page.evaluate("document.documentElement.scrollWidth") == 390
        page.keyboard.press("Escape")
        page.set_viewport_size({"width": 1440, "height": 1000})
        page.wait_for_timeout(100)
        checks.append("compare: a renamed file is one moved entry; a level new since the snapshot says so; a missing snapshot is an error; without snapshots the control says to run archgraph snapshot; off again draws nothing extra; ?compare= turns it on; the panel fits at phone width")

        screenshot = os.environ.get("ARCHGRAPH_UI_SCREENSHOT")
        if screenshot:
            page.screenshot(path=screenshot, full_page=True)
        page.evaluate("loadFocus('missing')")
        page.locator("#error").wait_for(state="visible")
        assert "unknown architecture node" in page.locator("#error").inner_text()
        checks.append("actionable unknown-focus errors")
        assert not errors, errors
        browser.close()
    print(json.dumps({"status": "passed", "scope": "Isolated browser DOM with mocked fetch/history; no real URL navigation or Rust server was tested", "checks": checks}, indent=2))


if __name__ == "__main__":
    main()
