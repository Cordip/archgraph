#!/usr/bin/env python3
"""Optional UI-only browser smoke test in an isolated DOM with mocked fetch/history interfaces.

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
            "descendant_file_count": max(len(files), 2), "interfaces": []}


NODES = {n["id"]: n for n in [
    node("app", "Application", ["app.api", "app.domain"]),
    node("app.api", "API", files=["src/api/a.rs"]),
    node("app.domain", "Domain", files=["src/domain/a.rs", "src/domain/b.rs"]),
    node("external", "External", ["external.service"], kind="external"),
    node("external.service", "Service", kind="external"),
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
}
META = {"project": {"name": "Browser fixture", "root": "app"}, "provider": {"provider": "fixture"}, "stats": {},
        "schema_version": 1, "evidence_notice": NOTICE, "diagnostics": ["Test-only coverage warning"], "read_only": True}


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
        mocked = {"meta": META, "projections": PROJECTIONS, "nodes": list(NODES.values())}
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
        # Layered layout: the dependent (API) is drawn above its dependency.
        api = page.get_by_role("button", name="API", exact=True).bounding_box()
        domain = page.get_by_role("button", name="Domain", exact=True).bounding_box()
        assert api["y"] < domain["y"], (api, domain)
        checks.append("root projection, directed arrows, merged relation kinds, external/manual entries, violation markers")

        page.locator("#graph .edge-label").first.click()
        assert "src/api/a.rs" in page.locator("#details").inner_text()
        assert "CALLS × 25" in page.locator("#details").inner_text()
        assert "IMPORTS × 25" in page.locator("#details").inner_text()
        assert "<img src=x" in page.locator("#details").inner_text()
        assert page.locator("#details img").count() == 0
        assert page.evaluate("window.__injected") is None
        assert page.locator("script").count() == 1
        checks.append("edge evidence and hostile provider text rendered without HTML execution")

        page.get_by_role("button", name="Domain", exact=True).dblclick()
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")
        assert page.locator("#graph .file").count() == 2
        assert "Domain service" in page.locator("#interfaces").inner_text()
        assert "focus=app.domain" in page.evaluate("window.__history.at(-1)")
        checks.append("double-click focus, leaf files, interfaces, updated deep link")

        page.locator("#breadcrumbs").get_by_role("link", name="Application", exact=True).click()
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app'")
        page.evaluate("loadFocus('app.domain')")
        page.evaluate("window.dispatchEvent(new PopStateEvent('popstate'))")
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app'")
        checks.append("breadcrumb navigation and popstate handler with mocked history")

        page.locator("#search").fill("Domain")
        page.locator("#search-results").get_by_role("button", name="Domain — app.domain", exact=True).click()
        page.wait_for_function("document.getElementById('focus-id').textContent === 'app.domain'")
        page.locator(".violation-button").click()
        assert "deny-api-domain" in page.locator("#details").inner_text()
        assert "src/api/a.rs" in page.locator("#details").inner_text()
        checks.append("architecture search and violation evidence selection")

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
        assert page.locator("#snapshot").inner_text() == "Live · revision 2"
        assert "Reloaded coverage warning" in page.locator("#diagnostics").inner_text()
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
