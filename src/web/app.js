"use strict";

// No HTML from architecture/provider data is interpreted. All dynamic content
// uses textContent; SVG is built with createElementNS and constant attribute keys.
const $ = (id) => document.getElementById(id);
const SVG_NS = "http://www.w3.org/2000/svg";
let currentProjection = null;
let meta = null;
let focusRequest = 0;
let searchRequest = 0;
let searchTimer = null;

function html(tag, text, className) {
  const element = document.createElement(tag);
  if (text !== undefined && text !== null) element.textContent = String(text);
  if (className) element.className = className;
  return element;
}
function svg(tag, attributes = {}, text) {
  const element = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, String(value));
  if (text !== undefined) element.textContent = String(text);
  return element;
}
async function api(path) {
  const response = await fetch(path, { headers: { Accept: "application/json" }, cache: "no-store" });
  const payload = await response.json();
  if (!response.ok) throw new Error(payload.error || `HTTP ${response.status}`);
  return payload;
}
function showError(error) { $("error").textContent = error.message || String(error); $("error").hidden = false; }
function focusUrl(id) { const url = new URL(location.href); url.searchParams.set("focus", id); return url; }
function linkTo(id, title) {
  const link = html("a", title);
  link.href = focusUrl(id).href;
  link.addEventListener("click", (event) => { event.preventDefault(); loadFocus(id); });
  return link;
}
function interfaceText(item) {
  return [item.name, item.direction, item.kind + (item.protocol ? `/${item.protocol}` : ""), item.contract].filter(Boolean).join(" · ");
}
function interfacesInto(parent, items) {
  for (const item of items || []) {
    const badge = html("div", interfaceText(item), "interface");
    if (item.description) badge.title = item.description;
    parent.append(badge);
  }
}
function detailsTitle(title, id) {
  const panel = $("details");
  panel.replaceChildren(html("h2", title));
  if (id) panel.append(html("p", id, "detail-id"));
  return panel;
}
function select(element) {
  for (const previous of document.querySelectorAll("#graph .selected")) previous.classList.remove("selected");
  if (element) element.classList.add("selected");
}
function addEvidence(parent, evidence, total) {
  for (const item of evidence || []) {
    const row = html("div", null, "evidence-item");
    row.append(html("div", `${item.from_file}\n→ ${item.to_file}`, "evidence-pair"));
    row.append(html("div", [item.kind, item.confidence === null || item.confidence === undefined ? null : `confidence ${item.confidence}`, item.reason].filter(Boolean).join(" · "), "evidence-meta"));
    parent.append(row);
  }
  const count = (evidence || []).length;
  if (count < total) parent.append(html("p", `Showing ${count} of ${total} observations, sorted deterministically. Use archgraph context with --evidence-limit for more.`, "notice"));
}
function showNode(node, element) {
  select(element);
  const panel = detailsTitle(node.title, node.file_path || node.architecture_id || node.id);
  const observed = node.observed_file_count === null || node.observed_file_count === undefined ? "" : ` (${node.observed_file_count} observed)`;
  panel.append(html("p", `${node.file_count} mapped file(s)${observed}${node.outside_focus ? " · outside current focus" : ""}`));
  if (node.description) panel.append(html("p", node.description));
  if ((node.interfaces || []).length) { panel.append(html("h3", "Interfaces")); interfacesInto(panel, node.interfaces); }
  if (node.entry_kind === "architecture") {
    const button = html("button", "Open architecture node");
    button.addEventListener("click", () => loadFocus(node.architecture_id));
    panel.append(button);
  }
  if (node.entry_kind === "file") panel.append(html("p", "File identity only. Use GitNexus or your editor for source and symbol details."));
  if (node.entry_kind === "direct_files") {
    panel.append(html("h3", "Directly owned files"));
    for (const file of currentProjection.focus.direct_files) panel.append(html("p", file, "detail-id"));
  }
  if (node.violation_rule_ids.length) {
    panel.append(html("h3", "⚠ Rules with violations"));
    for (const id of node.violation_rule_ids) panel.append(html("p", id, "violation-badge"));
  }
}
function displayEndpoint(id) {
  const node = currentProjection.nodes.find((entry) => entry.id === id);
  return node ? node.file_path || node.architecture_id || node.title : id;
}
// One drawn edge per endpoint pair and origin. Relation kinds (CALLS, IMPORTS,
// ...) between the same two nodes are listed in its details instead of being
// drawn as parallel arrows with overlapping labels.
function mergeEdges(edges) {
  const merged = new Map();
  for (const edge of edges) {
    const key = [edge.from, edge.to, edge.origin].join("\u0000");
    if (!merged.has(key)) merged.set(key, { from: edge.from, to: edge.to, origin: edge.origin, count: 0, parts: [] });
    const group = merged.get(key);
    group.count += edge.count;
    group.parts.push(edge);
  }
  const union = (parts, field) => [...new Set(parts.flatMap((part) => part[field] || []))].sort();
  return [...merged.values()].map((group) => ({ ...group,
    violation_rule_ids: union(group.parts, "violation_rule_ids"),
    suggested_cut_rule_ids: union(group.parts, "suggested_cut_rule_ids") }));
}
function edgeSummary(group) {
  return group.parts.length === 1 ? `${compact(group.parts[0].kind, 22)} × ${group.count}` : `${group.parts.length} kinds × ${group.count}`;
}
function showEdge(group, element) {
  select(element);
  const panel = detailsTitle(edgeSummary(group));
  panel.append(html("p", `${displayEndpoint(group.from)}\n→ ${displayEndpoint(group.to)}`, "evidence-pair"));
  panel.append(html("p", group.origin === "manual" ? "Manual relationship: descriptive intent, not source evidence." : "Observed file dependencies from GitNexus."));
  if (group.violation_rule_ids.length) panel.append(html("p", `⚠ ${group.violation_rule_ids.join(", ")}`, "violation-badge"));
  if (group.suggested_cut_rule_ids.length) panel.append(html("p", `✂ Suggested cut for ${group.suggested_cut_rule_ids.join(", ")}: removing this upward dependency helps break the cycle at the lowest observed cost.`, "cut-badge"));
  for (const edge of group.parts) {
    panel.append(html("h3", `${edge.kind} × ${edge.count}`));
    if (edge.confidence_min !== null && edge.confidence_min !== undefined) panel.append(html("p", `Confidence range: ${edge.confidence_min}–${edge.confidence_max}`));
    for (const manual of edge.manual_edges || []) {
      panel.append(html("p", manual.label || manual.id, "detail-id"));
      panel.append(html("p", `${manual.from} → ${manual.to}`, "detail-id"));
      if (manual.description) panel.append(html("p", manual.description));
    }
    if (edge.origin === "observed") addEvidence(panel, edge.evidence, edge.count);
  }
}
function showViolation(violation) {
  select(null);
  const panel = detailsTitle(`⚠ ${violation.rule_id}`);
  panel.append(html("p", violation.message));
  if (violation.kind === "no_cycles") panel.append(html("p", "These nodes form a strongly connected component, not necessarily a cycle in the displayed order."));
  const cuts = violation.suggested_cuts || [];
  const isCut = (edge) => cuts.some((cut) => cut.from === edge.from && cut.to === edge.to);
  if (cuts.length) {
    panel.append(html("h3", "✂ Suggested cut"));
    panel.append(html("p", `Layers, upper to lower: ${violation.layer_order.join(" > ")}`, "detail-id"));
    for (const cut of cuts) panel.append(html("p", `${cut.from} → ${cut.to} × ${cut.count}`, "cut-badge"));
  }
  for (const edge of violation.architecture_edges.filter((edge) => !cuts.length || isCut(edge))) {
    panel.append(html("h3", `${edge.from} → ${edge.to}`));
    panel.append(html("p", `${edge.kind} × ${edge.count}`));
    addEvidence(panel, edge.evidence, edge.count);
  }
  const others = violation.architecture_edges.filter((edge) => cuts.length && !isCut(edge));
  if (others.length) {
    panel.append(html("h3", "Other dependencies in the cycle"));
    for (const edge of others) panel.append(html("p", `${edge.from} → ${edge.to} · ${edge.kind} × ${edge.count}`, "detail-id"));
  }
}
function compact(value, length) {
  const text = String(value);
  return text.length <= length ? text : text.slice(0, length - 1) + "…";
}

// Rows from upper to lower layer (projection.layers, computed by the server so
// that dependencies mostly point down), else a grid. Outside entries that only
// depend on this focus get a band above it; all other outside entries a band
// below. Layout uses only this focus projection, never the repository graph.
function layout(projection) {
  const cardWidth = 205, cardHeight = 76, gapX = 100, gapY = 130, maxColumns = 5;
  const chunk = (ids, size) => { const rows = []; for (let i = 0; i < ids.length; i += size) rows.push(ids.slice(i, i + size)); return rows; };
  const inside = projection.nodes.filter((node) => !node.outside_focus).map((node) => node.id);
  const outside = projection.nodes.filter((node) => node.outside_focus);
  const callers = outside.filter((node) => !projection.edges.some((edge) => edge.to === node.id)).map((node) => node.id);
  const others = outside.map((node) => node.id).filter((id) => !callers.includes(id));
  const gridColumns = Math.max(1, Math.min(4, Math.ceil(Math.sqrt(inside.length || 1))));
  const insideRows = (projection.layers || []).length
    ? projection.layers.flatMap((layer) => chunk(layer, maxColumns))
    : chunk(inside, gridColumns);
  const bands = [
    { rows: chunk(callers, maxColumns), note: "OUTSIDE · DEPENDS ON THIS FOCUS" },
    { rows: insideRows, note: null },
    { rows: chunk(others, maxColumns), note: "OUTSIDE CURRENT FOCUS" },
  ].filter((band) => band.rows.length);
  const columns = Math.max(1, ...bands.flatMap((band) => band.rows.map((row) => row.length)));
  const width = Math.max(730, columns * (cardWidth + gapX) + 20);
  const positions = new Map(), notes = [];
  let y = 60;
  for (const band of bands) {
    if (band.note) { notes.push({ y: y - 14, text: band.note }); y += 10; }
    for (const row of band.rows) {
      // Centre each row so a narrow layer does not hug the left edge.
      const offset = 60 + (columns - row.length) * (cardWidth + gapX) / 2;
      row.forEach((id, index) => positions.set(id, { x: offset + index * (cardWidth + gapX), y, width: cardWidth, height: cardHeight }));
      y += cardHeight + gapY;
    }
    y += 20;
  }
  return { positions, width, height: Math.max(300, y - gapY + 45), notes };
}
function boundaryPoint(box, toward) {
  const cx = box.x + box.width / 2, cy = box.y + box.height / 2;
  const dx = toward.x - cx, dy = toward.y - cy;
  const scale = Math.min(dx === 0 ? Infinity : (box.width / 2 + 6) / Math.abs(dx), dy === 0 ? Infinity : (box.height / 2 + 6) / Math.abs(dy));
  return { x: cx + dx * scale, y: cy + dy * scale };
}
function drawGraph(projection) {
  const graph = $("graph");
  graph.replaceChildren();
  const { positions, width, height, notes } = layout(projection);
  graph.setAttribute("viewBox", `0 0 ${width} ${height}`);
  graph.setAttribute("height", height);
  graph.setAttribute("aria-label", `${projection.focus.title}: ${projection.nodes.length} nodes, ${projection.edges.length} directed dependencies`);
  const defs = svg("defs");
  for (const [id, className] of [["arrow", "arrow"], ["arrow-error", "arrow-error"]]) {
    const marker = svg("marker", { id, viewBox: "0 0 10 10", refX: 9, refY: 5, markerWidth: 7, markerHeight: 7, orient: "auto-start-reverse" });
    marker.append(svg("path", { d: "M 0 0 L 10 5 L 0 10 z", class: className }));
    defs.append(marker);
  }
  graph.append(defs);
  if (!projection.nodes.length) graph.append(svg("text", { x: 40, y: 70, class: "graph-note" }, "No mapped files or dependencies at this focus."));
  for (const note of notes) graph.append(svg("text", { x: 60, y: note.y, class: "graph-note" }, note.text));
  const laneCounts = new Map();
  for (const edge of mergeEdges(projection.edges)) {
    const from = positions.get(edge.from), to = positions.get(edge.to);
    if (!from || !to) throw new Error("Projection edge references an absent visible node");
    const key = [edge.from, edge.to].sort().join("\u0000");
    const lane = laneCounts.get(key) || 0;
    laneCounts.set(key, lane + 1);
    const a = { x: from.x + from.width / 2, y: from.y + from.height / 2 };
    const b = { x: to.x + to.width / 2, y: to.y + to.height / 2 };
    const distance = Math.hypot(b.x - a.x, b.y - a.y) || 1;
    // Edges that skip rows would run straight through the cards in between;
    // bend them further the more they skip.
    const bend = 24 + lane * 26 + Math.max(0, distance - 220) * 0.3;
    const control = { x: (a.x + b.x) / 2 - (b.y - a.y) / distance * bend,
      y: (a.y + b.y) / 2 + (b.x - a.x) / distance * bend };
    const start = boundaryPoint(from, control), end = boundaryPoint(to, control);
    const path = `M ${start.x} ${start.y} Q ${control.x} ${control.y} ${end.x} ${end.y}`;
    const violating = edge.violation_rule_ids.length > 0;
    const cut = edge.suggested_cut_rule_ids.length > 0;
    const group = svg("g", { class: `edge ${edge.origin}${violating ? " violating" : ""}${cut ? " cut" : ""}`, tabindex: 0, role: "button",
      "aria-label": `${displayEndpoint(edge.from)} to ${displayEndpoint(edge.to)}, ${edge.parts.map((part) => part.kind).join(", ")}, ${edge.count} ${edge.origin} relationships` });
    group.append(svg("path", { d: path, class: "edge-hit" }));
    group.append(svg("path", { d: path, class: "edge-line", "marker-end": `url(#${violating ? "arrow-error" : "arrow"})` }));
    const labelX = .25 * start.x + .5 * control.x + .25 * end.x;
    const labelY = .25 * start.y + .5 * control.y + .25 * end.y - 7;
    const label = `${edgeSummary(edge)}${edge.origin === "manual" ? " manual" : ""}${cut ? " ✂" : violating ? " ⚠" : ""}`;
    group.append(svg("text", { x: labelX, y: labelY, class: "edge-label" }, label));
    group.append(svg("title", {}, `${edge.parts.map((part) => `${part.kind} × ${part.count}`).join(", ")}. Click for concrete evidence.`));
    group.addEventListener("click", () => showEdge(edge, group));
    group.addEventListener("keydown", (event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); showEdge(edge, group); } });
    graph.append(group);
  }
  for (const node of projection.nodes) {
    const box = positions.get(node.id);
    const group = svg("g", { class: `node ${node.entry_kind}${node.outside_focus ? " outside" : ""}${node.violation_rule_ids.length ? " violating" : ""}`,
      transform: `translate(${box.x}, ${box.y})`, tabindex: 0, role: "button", "aria-label": node.title });
    group.append(svg("rect", { width: box.width, height: box.height, rx: 8 }));
    const title = `${node.violation_rule_ids.length ? "⚠ " : ""}${compact(node.title, 25)}`;
    group.append(svg("text", { x: 13, y: 27, class: "node-title" }, title));
    group.append(svg("text", { x: 13, y: 46, class: "node-subtitle" }, compact(node.file_path || node.architecture_id || node.entry_kind, 32)));
    group.append(svg("text", { x: 13, y: 63, class: "node-subtitle" }, `${node.file_count} file(s)${node.observed_file_count === null || node.observed_file_count === undefined ? "" : ` · ${node.observed_file_count} observed`}${node.outside_focus ? " · outside" : ""}${node.node_kind === "external" ? " · external" : ""}`));
    group.append(svg("title", {}, `${node.title}\n${node.architecture_id || node.file_path || ""}\n${node.description || ""}`));
    group.addEventListener("click", () => showNode(node, group));
    group.addEventListener("dblclick", () => { if (node.entry_kind === "architecture") loadFocus(node.architecture_id); });
    group.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") { event.preventDefault(); showNode(node, group); }
    });
    graph.append(group);
  }
}
async function loadFocus(id, pushHistory = true) {
  const request = ++focusRequest;
  $("loading").hidden = false;
  $("error").hidden = true;
  try {
    const projection = await api(`/api/focus/${encodeURIComponent(id)}`);
    if (request !== focusRequest) return;
    currentProjection = projection;
    if (pushHistory) history.pushState({ focus: id }, "", focusUrl(id));
    $("breadcrumbs").replaceChildren();
    for (const [index, ancestor] of projection.breadcrumbs.entries()) {
      if (index) $("breadcrumbs").append(html("span", " / "));
      $("breadcrumbs").append(linkTo(ancestor.id, ancestor.title));
    }
    $("focus-id").textContent = projection.focus.id;
    $("focus-title").textContent = projection.focus.title;
    $("focus-count").textContent = `${projection.focus.descendant_file_count} mapped files, ${projection.focus.observed_file_count} with observed dependencies`;
    $("description").textContent = projection.focus.description || "No description authored for this node.";
    $("interfaces").replaceChildren();
    interfacesInto($("interfaces"), projection.focus.interfaces);
    $("summary").replaceChildren(
      html("span", `${projection.nodes.filter((node) => !node.outside_focus).length} visible entries`),
      html("span", `${mergeEdges(projection.edges).length} dependencies (${projection.edges.length} by relation kind)`),
      html("span", `${projection.violations.length} violations touching this view`)
    );
    $("evidence-notice").textContent = projection.evidence_notice;
    drawGraph(projection);
    $("violations").replaceChildren();
    if (!projection.violations.length) $("violations").append(html("p", "No matching observed architecture violations."));
    for (const violation of projection.violations) {
      const button = html("button", `⚠ ${violation.rule_id} — ${violation.message}`, "violation-button");
      button.addEventListener("click", () => showViolation(violation));
      $("violations").append(button);
    }
    detailsTitle("Details & evidence").append(html("p", "Select a node, dependency, or violation."));
    document.title = `${projection.focus.title} · ArchGraph`;
  } catch (error) { if (request === focusRequest) showError(error); }
  finally { if (request === focusRequest) $("loading").hidden = true; }
}
async function searchNodes() {
  const request = ++searchRequest;
  const query = $("search").value.trim();
  $("search-results").replaceChildren();
  if (!query) return;
  try {
    const results = await api(`/api/search?q=${encodeURIComponent(query)}`);
    if (request !== searchRequest) return;
    if (!results.length) $("search-results").append(html("p", "No matching architecture nodes."));
    for (const node of results) {
      const button = html("button", `${node.title} — ${node.id}`);
      button.type = "button";
      button.addEventListener("click", () => {
        ++searchRequest;
        $("search-results").replaceChildren();
        $("search").value = "";
        loadFocus(node.id);
      });
      $("search-results").append(button);
    }
  } catch (error) { if (request === searchRequest) showError(error); }
}
$("search").addEventListener("input", () => { ++searchRequest; clearTimeout(searchTimer); searchTimer = setTimeout(searchNodes, 180); });
$("search-form").addEventListener("submit", (event) => { event.preventDefault(); clearTimeout(searchTimer); searchNodes(); });
$("search").addEventListener("keydown", (event) => { if (event.key === "Escape") { ++searchRequest; $("search-results").replaceChildren(); } });
window.addEventListener("popstate", () => { if (meta) loadFocus(new URL(location.href).searchParams.get("focus") || meta.project.root, false); });
(async () => {
  try {
    meta = await api("/api/meta");
    $("diagnostics-section").hidden = !meta.diagnostics.length;
    for (const warning of meta.diagnostics) $("diagnostics").append(html("p", warning));
    await loadFocus(new URL(location.href).searchParams.get("focus") || meta.project.root, false);
  } catch (error) { showError(error); }
})();
