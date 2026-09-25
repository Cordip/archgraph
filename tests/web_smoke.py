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
]}
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
}
META = {"project": {"name": "Browser fixture", "root": "app"}, "provider": {"provider": "fixture"}, "stats": {},
        "schema_version": 1, "evidence_notice": NOTICE, "diagnostics": ["Test-only coverage warning"], "read_only": True}


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
        mocked = {"meta": META, "projections": PROJECTIONS, "nodes": list(NODES.values()), "violations": [VIOLATION]}
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
        assert page.locator("script").count() == 1
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
        page.locator("#details .dependency", has_text="app.domain").click()
        assert "CALLS × 25" in page.locator("#details").inner_text()
        assert "Observed dependency" in page.locator("#details").inner_text()
        page.keyboard.press("Escape")
        assert "This level" in page.locator("#details").inner_text()
        checks.append("keyboard navigation between entries, highlighted neighbourhood, dependency list to edge evidence, Escape back to the level")

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
        assert page.locator("#tree [role='treeitem']").count() == 5
        assert page.locator("#tree [data-id='app.api'] .count-badge").inner_text() == "1"
        page.locator("#tree-violations").check()
        assert page.locator("#tree [role='treeitem']").count() == 3
        page.locator("#tree-violations").uncheck()
        page.locator("#tree [data-id='app.domain'] > .tree-row").click()
        assert page.locator("#details h2").inner_text() == "Domain"
        assert page.locator("#graph .node.selected").get_attribute("aria-label") == "Domain"
        checks.append("sidebar tree with violation badges, violations-only toggle, click selects and centres the entry")

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
