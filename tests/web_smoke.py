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
SEGMENTS = """() => [...document.querySelectorAll('#graph .edge-line, #graph .trunk-line')].map((line) => {
    const numbers = line.getAttribute('d').match(/-?[\\d.]+/g).map(Number);
    const points = [];
    for (let i = 0; i + 1 < numbers.length; i += 2) points.push([numbers[i], numbers[i + 1]]);
    return points;
})"""


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
    """The canvas camera as (x, y, k), parsed from the #graph transform."""
    numbers = [float(n) for n in re.findall(r"-?[\d.]+", page.locator("#graph").get_attribute("transform"))]
    return numbers[0], numbers[1], numbers[2]


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
        mocked = {"meta": META, "projections": PROJECTIONS, "nodes": list(NODES.values()), "violations": [VIOLATION], "packages": PACKAGES}
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

        page.locator("#graph .edge-label", has_text="2 kinds").click()
        assert "src/api/a.rs" in page.locator("#details").inner_text()
        assert "CALLS × 25" in page.locator("#details").inner_text()
        assert "IMPORTS × 25" in page.locator("#details").inner_text()
        assert "<img src=x" in page.locator("#details").inner_text()
        assert page.locator("#details img").count() == 0
        assert page.evaluate("window.__injected") is None
        assert page.locator("script").count() == 3
        checks.append("edge evidence and hostile provider text rendered without HTML execution")

        # The canvas is one tab stop: arrow keys move between entries, Enter
        # shows details, whose dependency list reaches the edges by keyboard.
        assert page.locator("#graph .node[tabindex='0']").count() == 1
        page.locator("#graph .node[tabindex='0']").focus()
        start = page.evaluate("document.activeElement.getAttribute('aria-label')")
        page.keyboard.press("ArrowDown")
        moved = page.evaluate("document.activeElement.getAttribute('aria-label')")
        assert moved != start and page.locator("#graph .node[tabindex='0']").count() == 1, (start, moved)
        page.get_by_role("button", name="API", exact=True).focus()
        page.keyboard.press("Enter")
        assert "Depends on (2)" in page.locator("#details").inner_text()
        # Selecting an entry dims everything it is not connected to.
        assert page.locator("#graph.has-selection").count() == 1
        assert page.locator("#graph .node.lit").count() == 3
        # A row in the details and its entry on the canvas light each other.
        row = page.locator("#details .dependency", has_text="app.domain")
        row.hover()
        assert page.locator("#graph.has-hover").count() == 1
        assert page.locator("#graph .node.hover").evaluate_all("els => els.map(e => e.getAttribute('aria-label')).sort()") == ["API", "Domain"]
        page.mouse.move(0, 0)
        assert page.locator("#graph.has-hover").count() == 0
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
        stage = page.locator("#stage").bounding_box()
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
        x2, y2, k2 = camera(page)
        assert k2 > k1 * 1.3, (k1, k2)
        assert abs((px - x2) / k2 - world[0]) < 0.5 and abs((py - y2) / k2 - world[1]) < 0.5
        assert page.locator("#zoom-level").inner_text() == f"{round(k2 * 100)}%"
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
        page.wait_for_function("document.querySelectorAll('#graph .node.file').length === 200")
        assert page.locator("#collapse-groups").is_visible()
        page.locator("#collapse-groups").click()
        assert page.locator("#graph .node.group").count() == 1
        page.locator("#view-table").click()
        assert page.locator("#table-wrap .entry-row").count() == 200
        page.locator("#view-diagram").click()
        checks.append("large levels as expandable directory groups; the table lists every file")

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
        page.locator("#stage").click(position={"x": 20, "y": 500})
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
        assert "imported by 2 files" in page.locator(f"#graph .node.package[data-id='{ORTOOLS['id']}']").text_content()
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
        checks.append("PCB mode: every segment at a multiple of 45°, parallel traces at least a pitch apart, deterministic routing, a dragged card snaps to the grid and re-routes, positions stored per mode")

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
        page.locator("#graph .node[data-id='file:w/a.ts']").hover()
        assert page.locator("#graph .edge.hover .edge-label").first.evaluate("e => getComputedStyle(e).display") != "none"
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

        # Wires from several siblings into one target merge into a trunk: one
        # arrowhead instead of six, a tag with the count, a red casing and a
        # count when one of its wires violates a rule, in every mode.
        page.evaluate("(p) => { window.__fixture.projections['app.trunks'] = p; }", trunk_projection())
        page.evaluate("loadFocus('app.trunks')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.trunks'")
        into_service = """() => [...document.querySelectorAll('#graph [marker-end]')].filter((path) => {
            const edge = path.closest('.edge'); return edge.classList.contains('trunk') || scene.edgeEls.find((item) => item.element === edge).edge.to === 'node:external.service'; }).length"""
        for mode in ("curves", "pcb", "hex"):
            page.locator(f"#mode-{mode}").click()
            page.locator("#zoom-fit").click()
            assert page.locator("#graph .edge.trunk").count() == 1, mode
            assert page.locator("#graph .edge.branch").count() == 6, mode
            assert page.evaluate(into_service) == 1, mode
            tag = page.locator("#graph .edge.trunk .tag-full .tag-text").text_content()
            assert tag.startswith("×6 → Service") and "⚠ 1" in tag, (mode, tag)
            assert page.locator("#graph .edge.trunk .tag-short .tag-text").text_content().startswith("×6"), mode
            trunk = page.locator("#graph .edge.trunk")
            assert "violating" in trunk.get_attribute("class")
            assert trunk.locator(".edge-casing").first.evaluate("e => getComputedStyle(e).stroke") == "rgb(196, 34, 27)", mode
            # Wider where more wires share it.
            widths = trunk.locator(".trunk-line").evaluate_all("els => els.map((e) => parseFloat(getComputedStyle(e).strokeWidth))")
            assert max(widths) >= 4.5 and max(widths) > min(widths), (mode, widths)
            # With colours by source a trunk of several sources is neutral ink.
            assert "neutral" in trunk.get_attribute("class"), mode
            if mode != "curves":
                assert not angles_ok(page, 45 if mode == "pcb" else 60), (mode, angles_ok(page, 45 if mode == "pcb" else 60))
                first = page.evaluate(SEGMENTS)
                page.evaluate("redraw()")
                assert page.evaluate(SEGMENTS) == first, mode
            if mode == "pcb":
                assert not spacing_problems(page, 12), spacing_problems(page, 12)
            # Hovering the trunk lights its six wires and their entries.
            trunk.locator(".trunk-tag").hover()
            assert page.locator("#graph .edge.branch.hover").count() == 6, mode
            assert page.locator("#graph .node.hover").count() == 7, mode
            page.mouse.move(0, 0)
        page.locator("#graph .edge.trunk .trunk-tag").click()
        details = page.locator("#details").inner_text()
        assert "Trunk" in details and "Wires in this trunk (6)" in details and "1 of them violates deny-api-domain" in details, details
        page.locator("#details .dependency", has_text="t/s3.ts").click()
        assert "Observed dependency" in page.locator("#details").inner_text()
        assert page.locator("#graph .edge.branch.selected").count() == 1
        # Coloured by target, the trunk takes the target's colour.
        page.locator("#colour-by").select_option("target")
        assert "neutral" not in page.locator("#graph .edge.trunk").get_attribute("class")
        assert re.search(r"\bn\d\b", page.locator("#graph .edge.trunk").get_attribute("class"))
        page.locator("#colour-by").select_option("source")
        page.locator("#mode-curves").click()
        page.evaluate("loadFocus('app.domain')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")
        checks.append("a trunk replaces six parallel wires into one target in curves, PCB and Hex: one arrowhead, tag with the count, wider where more wires share it, red casing and a count for a violating wire, hover lights its wires, details list them; neutral when sources differ, the target's colour when coloured by target")

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
        assert opacity(own) < 0.3 and opacity(other) < 0.3, (opacity(own), opacity(other))
        assert opacity("#graph .edge.violating") >= 0.75
        page.locator("#graph .node[data-id='file:d/f05.ts']").hover()
        page.wait_for_timeout(300)
        assert opacity(own) == 1 and opacity(other) < 0.3, (opacity(own), opacity(other))
        page.mouse.move(0, 0)
        page.locator("#focus-mode").click()
        page.wait_for_timeout(300)
        assert page.locator("#graph.faint").count() == 0 and opacity(other) == 1
        assert page.locator("#focus-mode").get_attribute("aria-pressed") == "false"
        assert page.evaluate("JSON.parse(localStorage.getItem('archgraph.view.v1')).focus") is False
        page.locator("#focus-mode").click()
        assert page.locator("#graph.faint").count() == 1
        page.evaluate("loadFocus('app.domain')")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")
        checks.append("focus mode: above 60 wires all are faint, an entry's own wires light up on hover, violations stay strong; the toggle is remembered")

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
