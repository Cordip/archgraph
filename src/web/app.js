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
// Levels with more entries than this are listed in the table only: a drawing
// of hundreds of files is neither readable nor fast.
const DIAGRAM_LIMIT = 150;
// Above this many drawn edges, labels appear only on highlighted edges.
const LABEL_LIMIT = 40;
let viewChoice = null; // "diagram" | "table" once the user picks one
let view = "diagram";
let zoom = { scale: 1, fit: true };
let drawing = null; // { width, height, nodes: Map(id → element), edges: [{ edge, element }] }
let selection = null; // entries and edges lit by the current selection
let entries = new Map(); // projection entry id → entry
let merged = []; // mergeEdges(currentProjection.edges)
const narrow = window.matchMedia("(max-width: 900px)");

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
function showDiagnostics() {
  $("diagnostics").replaceChildren();
  $("diagnostics-section").hidden = !meta.diagnostics.length;
  for (const warning of meta.diagnostics) $("diagnostics").append(html("li", warning));
}
function showRefreshState(unreachable) {
  $("project-name").textContent = meta.project.name || "";
  $("snapshot").textContent = meta.watching ? `Live at revision ${meta.revision}` : "Read-only snapshot";
  const problem = unreachable
    ? "Cannot reach archgraph serve; showing the last loaded state."
    : meta.refresh_error && `Showing revision ${meta.revision}; the latest reload failed: ${meta.refresh_error}`;
  $("snapshot").className = `snapshot${meta.watching ? " live" : ""}${problem ? " stale" : ""}`;
  $("refresh-status").textContent = problem || "";
  $("refresh-status").hidden = !problem;
}
// The server recompiles when the index or architecture.yaml changes; follow
// it, staying on the current node while it still exists.
async function checkForUpdates() {
  let latest;
  try { latest = await api("/api/meta"); } catch { showRefreshState(true); return false; }
  const changed = latest.revision !== meta.revision;
  meta = latest;
  showRefreshState(false);
  if (!changed) return false;
  showDiagnostics();
  let id = currentProjection ? currentProjection.focus.id : meta.project.root;
  try { await api(`/api/focus/${encodeURIComponent(id)}`); } catch { id = meta.project.root; }
  await loadFocus(id, false);
  return true;
}
function showError(error) { $("error").textContent = error.message || String(error); $("error").hidden = false; }
function focusUrl(id) { const url = new URL(location.href); url.searchParams.set("focus", id); return url; }
function linkTo(id, title) {
  const link = html("a", title);
  link.href = focusUrl(id).href;
  link.addEventListener("click", (event) => { event.preventDefault(); loadFocus(id); });
  return link;
}
function interfacesInto(parent, items) {
  for (const item of items || []) {
    const chip = html("div", null, "interface");
    chip.append(html("span", item.name, "interface-name"));
    for (const part of [item.direction, item.kind + (item.protocol ? `/${item.protocol}` : ""), item.contract].filter(Boolean)) {
      chip.append(html("span", part, "interface-part"));
    }
    if (item.description) chip.title = item.description;
    parent.append(chip);
  }
}
function detailsTitle(title, id, kind) {
  const panel = $("details");
  panel.replaceChildren();
  delete panel.dataset.entry;
  if (kind) panel.append(html("p", kind, "detail-kind"));
  panel.append(html("h2", title));
  if (id) panel.append(html("p", id, "detail-id"));
  return panel;
}
// On narrow screens the details panel sits below the drawing; bring it into
// view after the user picks something, or the click seems to do nothing.
function revealDetails() {
  if (!narrow.matches) return;
  const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  $("details").scrollIntoView({ block: "start", behavior: reduce ? "auto" : "smooth" });
}
// `lit`: { nodes: Set of entry ids, edges: Set of merged edges } to keep at
// full strength while everything else in the drawing is dimmed.
function select(element, lit) {
  for (const previous of document.querySelectorAll("#graph .selected, #table-wrap .selected, #violations .selected")) previous.classList.remove("selected");
  if (element) element.classList.add("selected");
  selection = lit || null;
  paint(selection, "lit", "has-selection");
}
function paint(lit, className, rootClass) {
  if (!drawing) return;
  $("graph").classList.toggle(rootClass, Boolean(lit));
  for (const [id, element] of drawing.nodes) element.classList.toggle(className, Boolean(lit) && lit.nodes.has(id));
  for (const item of drawing.edges) {
    const on = Boolean(lit) && lit.edges.has(item.edge);
    item.element.classList.toggle(className, on);
    const strong = item.element.classList.contains("lit") || item.element.classList.contains("hover");
    item.line.setAttribute("marker-end", strong && !item.violating ? "url(#arrow-lit)" : item.marker);
  }
}
function neighbourhood(id) {
  const lit = { nodes: new Set([id]), edges: new Set() };
  for (const edge of merged) {
    if (edge.from !== id && edge.to !== id) continue;
    lit.edges.add(edge);
    lit.nodes.add(edge.from);
    lit.nodes.add(edge.to);
  }
  return lit;
}
function addEvidence(parent, evidence, total) {
  const list = html("ol", null, "evidence");
  for (const item of evidence || []) {
    const row = html("li", null, "evidence-item");
    row.append(html("div", `${item.from_file}\n→ ${item.to_file}`, "evidence-pair"));
    const line = html("div", null, "evidence-meta");
    line.append(html("span", item.kind, "tag"));
    if (item.confidence !== null && item.confidence !== undefined) line.append(html("span", `confidence ${item.confidence}`));
    if (item.reason) line.append(html("span", item.reason, "reason"));
    row.append(line);
    list.append(row);
  }
  if (list.childElementCount) parent.append(list);
  const count = (evidence || []).length;
  if (count < total) parent.append(html("p", `Showing ${count} of ${total} observations, sorted deterministically. Use archgraph context with --evidence-limit for more.`, "notice"));
}
const ENTRY_KINDS = { architecture: "Architecture node", file: "File", direct_files: "Directly owned files", boundary: "Boundary of this focus" };
function entryKind(node) {
  const kind = node.node_kind === "external" ? "External architecture node" : ENTRY_KINDS[node.entry_kind] || node.entry_kind;
  return node.outside_focus ? `${kind}, outside this focus` : kind;
}
function plural(count, noun) { return `${count} ${noun}${count === 1 ? "" : "s"}`; }
function hasValue(value) { return value !== null && value !== undefined; }
function showNode(node, element) {
  select(element, neighbourhood(node.id));
  const title = node.entry_kind === "file" && node.file_path ? splitPath(node.file_path)[1] : node.title;
  const panel = detailsTitle(title, node.file_path || node.architecture_id || node.id, entryKind(node));
  if (node.entry_kind !== "file") {
    const observed = hasValue(node.observed_file_count) ? `, ${node.observed_file_count} with observed dependencies` : "";
    panel.append(html("p", `${plural(node.file_count, "mapped file")}${observed}`));
  }
  if (node.description) panel.append(html("p", node.description));
  if ((node.interfaces || []).length) {
    panel.append(html("h3", "Interfaces"));
    const chips = html("div", null, "interfaces");
    interfacesInto(chips, node.interfaces);
    panel.append(chips);
  }
  if (node.entry_kind === "architecture") {
    const button = html("button", "Open architecture node", "primary");
    button.type = "button";
    button.addEventListener("click", () => loadFocus(node.architecture_id));
    panel.append(button);
  }
  if (node.entry_kind === "file") panel.append(html("p", "File identity only. Use GitNexus or your editor for source and symbol details.", "muted"));
  if (node.entry_kind === "direct_files") {
    panel.append(html("h3", "Directly owned files"));
    const files = html("ul", null, "plain-list");
    for (const file of currentProjection.focus.direct_files) files.append(html("li", file, "detail-id"));
    panel.append(files);
  }
  if (node.violation_rule_ids.length) {
    panel.append(html("h3", "Rules with violations"));
    for (const id of node.violation_rule_ids) panel.append(html("p", id, "violation-badge"));
  }
  dependencyList(panel, "Depends on", merged.filter((edge) => edge.from === node.id), (edge) => edge.to);
  dependencyList(panel, "Used by", merged.filter((edge) => edge.to === node.id), (edge) => edge.from);
  revealDetails();
}
// Each dependency of the selected entry, as a button that opens its evidence:
// the keyboard route to edges, which are not tab stops in the drawing.
function dependencyList(panel, heading, edges, other) {
  if (!edges.length) return;
  const limit = 200;
  panel.append(html("h3", `${heading} (${edges.length})`));
  const list = html("ul", null, "dependency-list");
  const sorted = [...edges].sort((a, b) => b.count - a.count || displayEndpoint(other(a)).localeCompare(displayEndpoint(other(b))));
  for (const edge of sorted.slice(0, limit)) {
    const button = html("button", null, `dependency${edge.violation_rule_ids.length ? " violating" : ""}`);
    button.type = "button";
    button.append(html("span", displayEndpoint(other(edge)), "dependency-name"));
    button.append(html("span", `${edgeSummary(edge)}${edge.origin === "manual" ? " manual" : ""}${edge.violation_rule_ids.length ? " ⚠" : ""}`, "dependency-count"));
    button.addEventListener("click", () => {
      const drawn = drawing && drawing.edges.find((item) => item.edge === edge);
      showEdge(edge, drawn ? drawn.element : null);
    });
    const item = html("li");
    item.append(button);
    list.append(item);
  }
  panel.append(list);
  if (edges.length > limit) panel.append(html("p", `${edges.length - limit} more not listed.`, "notice"));
}
function displayEndpoint(id) {
  const node = entries.get(id);
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
  select(element, { nodes: new Set([group.from, group.to]), edges: new Set([group]) });
  const panel = detailsTitle(edgeSummary(group), null, group.origin === "manual" ? "Manual dependency" : "Observed dependency");
  panel.append(html("p", `${displayEndpoint(group.from)}\n→ ${displayEndpoint(group.to)}`, "evidence-pair endpoints"));
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
  revealDetails();
}
// Lights the entries and drawn edges this violation involves.
function violationScope(violation) {
  const lit = { nodes: new Set(), edges: new Set() };
  const affected = new Set(violation.affected_nodes || violation.nodes || []);
  for (const node of currentProjection.nodes) {
    if (node.entry_kind === "architecture" && affected.has(node.architecture_id)) lit.nodes.add(node.id);
  }
  for (const edge of merged) {
    if (!edge.violation_rule_ids.includes(violation.rule_id)) continue;
    lit.edges.add(edge);
    lit.nodes.add(edge.from);
    lit.nodes.add(edge.to);
  }
  return lit;
}
function showViolation(violation, element) {
  select(element, violationScope(violation));
  const panel = detailsTitle(violation.rule_id, null, `Violation: ${violation.kind.replaceAll("_", " ")}`);
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
    for (const edge of others) panel.append(html("p", `${edge.from} → ${edge.to}, ${edge.kind} × ${edge.count}`, "detail-id"));
  }
  revealDetails();
}
function compact(value, length) {
  const text = String(value);
  return text.length <= length ? text : text.slice(0, length - 1) + "…";
}
// Keeps the end of a long directory, which is the part that tells files apart.
function compactStart(value, length) {
  const text = String(value);
  return text.length <= length ? text : "…" + text.slice(text.length - length + 1);
}
function splitPath(path) {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? ["", path] : [path.slice(0, slash), path.slice(slash + 1)];
}

// Rows from upper to lower layer (projection.layers, computed by the server so
// that dependencies mostly point down), else a grid. Outside entries that only
// depend on this focus get a band above the focus frame; all other outside
// entries a band below. Within a row, entries are ordered by the mean position
// of their neighbours (a few deterministic barycentre sweeps), which removes
// most crossings. Layout uses only this focus projection.
const CARD = { width: 184, height: 68 }, GAP_X = 30, GAP_Y = 100, PAD = 30, MAX_COLUMNS = 6;
function layout(projection, edges) {
  const chunk = (ids, size) => { const rows = []; for (let i = 0; i < ids.length; i += size) rows.push(ids.slice(i, i + size)); return rows; };
  const inside = projection.nodes.filter((node) => !node.outside_focus).map((node) => node.id);
  const outside = projection.nodes.filter((node) => node.outside_focus);
  const callers = outside.filter((node) => !projection.edges.some((edge) => edge.to === node.id)).map((node) => node.id);
  const others = outside.map((node) => node.id).filter((id) => !callers.includes(id));
  const layers = (projection.layers || []).length
    ? projection.layers
    : chunk(inside, Math.max(1, Math.min(MAX_COLUMNS, Math.ceil(Math.sqrt(inside.length * 1.5)))));
  const groups = [{ ids: callers, band: "callers" }, ...layers.map((ids) => ({ ids, band: "inside" })), { ids: others, band: "others" }]
    .filter((group) => group.ids.length);
  const columns = Math.max(1, ...groups.map((group) => Math.min(group.ids.length, MAX_COLUMNS)));
  const rowWidth = (count) => count * CARD.width + (count - 1) * GAP_X;
  const innerWidth = rowWidth(columns);
  const neighbours = new Map();
  for (const edge of edges) {
    for (const [a, b] of [[edge.from, edge.to], [edge.to, edge.from]]) {
      if (!neighbours.has(a)) neighbours.set(a, []);
      neighbours.get(a).push(b);
    }
  }
  const centre = new Map();
  let rows = [];
  for (let sweep = 0; sweep < 4; sweep++) {
    rows = [];
    for (const group of groups) {
      const keyed = group.ids.map((id, index) => {
        const placed = (neighbours.get(id) || []).filter((other) => centre.has(other));
        const fallback = centre.has(id) ? centre.get(id) : (index + 0.5) / group.ids.length * innerWidth;
        return { id, index, key: placed.length ? placed.reduce((sum, other) => sum + centre.get(other), 0) / placed.length : fallback };
      });
      keyed.sort((a, b) => a.key - b.key || a.index - b.index);
      for (const row of chunk(keyed.map((item) => item.id), MAX_COLUMNS)) {
        const offset = (innerWidth - rowWidth(row.length)) / 2;
        row.forEach((id, index) => centre.set(id, offset + index * (CARD.width + GAP_X) + CARD.width / 2));
        rows.push({ ids: row, band: group.band });
      }
    }
  }
  const width = Math.max(560, innerWidth + 2 * (PAD + 20));
  const left = (width - innerWidth) / 2;
  const positions = new Map(), notes = [];
  let y = PAD, bottom = PAD, band = null, frame = null;
  rows.forEach((row, rowIndex) => {
    if (row.band !== band) {
      if (band === "inside") frame.bottom = bottom + 22;
      if (band) y = (band === "inside" ? frame.bottom : bottom) + 46;
      band = row.band;
      if (band === "inside") { frame = { top: y, bottom: 0 }; y += 40; }
      else { notes.push({ y: y + 10, text: band === "callers" ? "Outside this focus, depending on it" : "Outside this focus" }); y += 24; }
    }
    const offset = left + (innerWidth - rowWidth(row.ids.length)) / 2;
    row.ids.forEach((id, index) => positions.set(id, { x: offset + index * (CARD.width + GAP_X), y, width: CARD.width, height: CARD.height, row: rowIndex }));
    bottom = y + CARD.height;
    y += CARD.height + GAP_Y;
  });
  if (band === "inside") frame.bottom = bottom + 22;
  return { positions, width, height: Math.max(200, (band === "inside" ? frame.bottom : bottom) + PAD), notes, frame, left, innerWidth };
}
const round = (value) => Math.round(value * 10) / 10;
// Downward edges leave a card's bottom and enter the next card's top; upward
// ones (against the layer order) run top to bottom and bow to the side, so
// they stand out. Each card side spreads its edges over several ports, sorted
// by where the other end is, so bundles fan out instead of piling up.
function route(edges, positions) {
  const plans = edges.map((edge, order) => {
    const a = positions.get(edge.from), b = positions.get(edge.to);
    if (!a || !b) throw new Error("Projection edge references an absent visible node");
    return { edge, order, a, b, dir: b.row > a.row ? "down" : b.row < a.row ? "up" : "same" };
  });
  const ports = new Map();
  const attach = (id, box, side, plan, end, other) => {
    const key = `${id}\u0000${side}`;
    if (!ports.has(key)) ports.set(key, { box, side, items: [] });
    ports.get(key).items.push({ plan, end, other });
  };
  for (const plan of plans) {
    attach(plan.edge.from, plan.a, plan.dir === "down" ? "bottom" : "top", plan, "start", plan.b);
    attach(plan.edge.to, plan.b, plan.dir === "up" ? "bottom" : "top", plan, "end", plan.a);
  }
  for (const { box, side, items } of ports.values()) {
    items.sort((p, q) => p.other.x - q.other.x || p.plan.order - q.plan.order);
    items.forEach((item, index) => {
      const gap = item.end === "end" ? 2 : 0;
      item.plan[item.end] = { x: box.x + box.width * (0.12 + 0.76 * (index + 0.5) / items.length),
        y: side === "bottom" ? box.y + box.height + gap : box.y - gap };
    });
  }
  // An edge that skips rows passes each row in between through the gap
  // between two cards (or beside the row) nearest to its straight course,
  // instead of running across the cards.
  const rows = new Map();
  for (const box of positions.values()) {
    if (!rows.has(box.row)) rows.set(box.row, []);
    rows.get(box.row).push(box);
  }
  for (const list of rows.values()) list.sort((p, q) => p.x - q.x);
  const channelUse = new Map();
  const channel = (row, x) => {
    const boxes = rows.get(row);
    const gaps = [boxes[0].x - GAP_X / 2, ...boxes.map((box) => box.x + box.width + GAP_X / 2)];
    let best = 0;
    gaps.forEach((gap, index) => { if (Math.abs(gap - x) < Math.abs(gaps[best] - x)) best = index; });
    const key = `${row}:${best}`, used = channelUse.get(key) || 0;
    channelUse.set(key, used + 1);
    return gaps[best] + ((used % 5) - 2) * 4;
  };
  for (const plan of plans) {
    const s = plan.start, e = plan.end;
    plan.curves = [];
    if (plan.dir === "same") {
      const lift = 30 + Math.abs(e.x - s.x) * 0.14;
      plan.curves.push([s, { x: s.x, y: s.y - lift }, { x: e.x, y: e.y - lift }, e]);
      plan.d = `M ${round(s.x)} ${round(s.y)} ${curveTo(plan.curves[0])}`;
      continue;
    }
    const sign = plan.dir === "down" ? 1 : -1;
    const points = [s];
    for (let row = plan.a.row + sign; row !== plan.b.row; row += sign) {
      const box = rows.get(row)[0];
      const x = channel(row, s.x + (e.x - s.x) * (box.y + box.height / 2 - s.y) / (e.y - s.y));
      const enter = sign > 0 ? box.y - 10 : box.y + box.height + 10, leave = sign > 0 ? box.y + box.height + 10 : box.y - 10;
      points.push({ x, y: enter }, { x, y: leave });
    }
    points.push(e);
    let d = `M ${round(s.x)} ${round(s.y)}`;
    for (let i = 0; i + 1 < points.length; i += 2) {
      const p = points[i], q = points[i + 1];
      const dy = Math.max(30, Math.abs(q.y - p.y) * 0.5) * sign;
      // Upward edges between neighbouring rows bow to the side, so they do not
      // overlap a downward edge between the same two cards.
      const bow = plan.dir === "up" && points.length === 2 ? 40 : 0;
      const curve = [p, { x: p.x + bow, y: p.y + dy }, { x: q.x + bow, y: q.y - dy }, q];
      plan.curves.push(curve);
      d += ` ${curveTo(curve)}`;
      if (i + 2 < points.length) d += ` L ${round(points[i + 2].x)} ${round(points[i + 2].y)}`;
    }
    plan.d = d;
  }
  return plans;
}
function curveTo([, c1, c2, q]) {
  return `C ${round(c1.x)} ${round(c1.y)} ${round(c2.x)} ${round(c2.y)} ${round(q.x)} ${round(q.y)}`;
}
function bezier([p, c1, c2, q], t) {
  const u = 1 - t;
  return { x: u * u * u * p.x + 3 * u * u * t * c1.x + 3 * u * t * t * c2.x + t * t * t * q.x,
    y: u * u * u * p.y + 3 * u * u * t * c1.y + 3 * u * t * t * c2.y + t * t * t * q.y };
}
// Greedy label placement: heavier and violating edges first, each label at
// the first spot along its edge that overlaps no card and no earlier label.
// Labels that fit nowhere appear only when their edge is highlighted.
function placeLabels(plans, positions) {
  const taken = [...positions.values()].map((box) => ({ x: box.x - 2, y: box.y - 2, width: box.width + 4, height: box.height + 4 }));
  const overlaps = (a) => taken.some((b) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height);
  const order = [...plans].sort((a, b) => (b.edge.violation_rule_ids.length > 0) - (a.edge.violation_rule_ids.length > 0) || b.edge.count - a.edge.count || a.order - b.order);
  for (const plan of order) {
    const width = plan.text.length * 6.3 + 6, height = 14;
    const curves = plan.curves.length > 1 ? [plan.curves[0], plan.curves[plan.curves.length - 1]] : plan.curves;
    let fallback = null;
    plan.crowded = true;
    for (const t of [0.5, 0.35, 0.65, 0.22, 0.78]) {
      for (const curve of curves) {
        const point = bezier(curve, t);
        const box = { x: point.x - width / 2, y: point.y - 7, width, height };
        fallback = fallback || point;
        if (overlaps(box)) continue;
        taken.push(box);
        plan.label = point;
        plan.crowded = false;
        break;
      }
      if (!plan.crowded) break;
    }
    if (plan.crowded) plan.label = fallback;
  }
}
// A revision cloud: the scalloped outline drafters draw around a change.
// Here it marks entries that take part in a rule violation.
function cloudPath(x, y, width, height) {
  const parts = [`M ${round(x)} ${round(y)}`];
  const side = (x0, y0, x1, y1) => {
    const length = Math.hypot(x1 - x0, y1 - y0), count = Math.max(1, Math.round(length / 15));
    const radius = round(length / count * 0.6);
    for (let i = 1; i <= count; i++) {
      parts.push(`A ${radius} ${radius} 0 0 1 ${round(x0 + (x1 - x0) * i / count)} ${round(y0 + (y1 - y0) * i / count)}`);
    }
  };
  side(x, y, x + width, y);
  side(x + width, y, x + width, y + height);
  side(x + width, y + height, x, y + height);
  side(x, y + height, x, y);
  return `${parts.join(" ")} Z`;
}
function nodeLines(node) {
  if (node.entry_kind === "file" && node.file_path) {
    const [directory, base] = splitPath(node.file_path);
    return [compact(base, 24), directory ? compactStart(`${directory}/`, 32) : "", null];
  }
  const facts = [plural(node.file_count, "file")];
  if (hasValue(node.observed_file_count)) facts.push(`${node.observed_file_count} observed`);
  if (node.node_kind === "external") facts.push("external");
  return [compact(node.title, 24), compact(node.architecture_id || ENTRY_KINDS[node.entry_kind] || node.entry_kind, 32), facts.join(", ")];
}
function drawGraph(projection) {
  const graph = $("graph");
  graph.replaceChildren();
  const { positions, width, height, notes, frame, left, innerWidth } = layout(projection, merged);
  drawing = { width, height, positions, nodes: new Map(), edges: [] };
  graph.setAttribute("viewBox", `0 0 ${width} ${height}`);
  graph.setAttribute("class", merged.length > LABEL_LIMIT ? "quiet" : "");
  graph.setAttribute("aria-label", `${projection.focus.title}: ${projection.nodes.length} nodes, ${projection.edges.length} directed dependencies`);
  const defs = svg("defs");
  for (const id of ["arrow", "arrow-error", "arrow-lit"]) {
    const marker = svg("marker", { id, viewBox: "0 0 10 10", refX: 9, refY: 5, markerWidth: 9, markerHeight: 9, markerUnits: "userSpaceOnUse", orient: "auto-start-reverse" });
    marker.append(svg("path", { d: "M 0 0 L 10 5 L 0 10 z", class: id }));
    defs.append(marker);
  }
  graph.append(defs);
  if (!projection.nodes.length) graph.append(svg("text", { x: 40, y: 70, class: "graph-note" }, "No mapped files or dependencies at this focus."));
  if (frame) {
    graph.append(svg("rect", { x: left - 20, y: frame.top, width: innerWidth + 40, height: frame.bottom - frame.top, class: "frame" }));
    graph.append(svg("text", { x: left - 8, y: frame.top + 22, class: "frame-label" }, compact(projection.focus.title, 60)));
  }
  for (const note of notes) graph.append(svg("text", { x: left - 20, y: note.y, class: "graph-note" }, note.text));
  // Violating edges are drawn last, on top of the others.
  const plans = route(merged, positions).sort((a, b) => (a.edge.violation_rule_ids.length > 0) - (b.edge.violation_rule_ids.length > 0) || a.order - b.order);
  for (const plan of plans) {
    const edge = plan.edge;
    plan.text = `${edgeSummary(edge)}${edge.origin === "manual" ? " manual" : ""}${edge.suggested_cut_rule_ids.length ? " ✂" : edge.violation_rule_ids.length ? " ⚠" : ""}`;
  }
  placeLabels(plans, positions);
  for (const plan of plans) {
    const edge = plan.edge;
    const violating = edge.violation_rule_ids.length > 0;
    const cut = edge.suggested_cut_rule_ids.length > 0;
    const weight = edge.count > 50 ? 4 : edge.count > 10 ? 3 : edge.count > 2 ? 2 : 1;
    // Edges are not tab stops (a level can have hundreds); the keyboard
    // reaches them through the selected entry's dependency list.
    const group = svg("g", { class: `edge ${edge.origin} w${weight}${plan.dir === "up" ? " upward" : ""}${plan.crowded ? " crowded" : ""}${violating ? " violating" : ""}${cut ? " cut" : ""}`, "aria-hidden": "true" });
    group.append(svg("path", { d: plan.d, class: "edge-hit" }));
    const marker = `url(#${violating ? "arrow-error" : "arrow"})`;
    const line = svg("path", { d: plan.d, class: "edge-line", "marker-end": marker });
    group.append(line);
    group.append(svg("text", { x: round(plan.label.x), y: round(plan.label.y + 4), class: "edge-label" }, plan.text));
    group.append(svg("title", {}, `${displayEndpoint(edge.from)} → ${displayEndpoint(edge.to)}\n${edge.parts.map((part) => `${part.kind} × ${part.count}`).join(", ")}. Click for concrete evidence.`));
    group.addEventListener("click", () => showEdge(edge, group));
    group.addEventListener("mouseenter", () => paint({ nodes: new Set([edge.from, edge.to]), edges: new Set([edge]) }, "hover", "has-hover"));
    group.addEventListener("mouseleave", () => paint(null, "hover", "has-hover"));
    graph.append(group);
    drawing.edges.push({ edge, element: group, line, marker, violating });
  }
  projection.nodes.forEach((node, index) => {
    const box = positions.get(node.id);
    const violating = node.violation_rule_ids.length > 0;
    const group = svg("g", { class: `node ${node.entry_kind}${node.outside_focus ? " outside" : ""}${node.node_kind === "external" ? " external" : ""}${violating ? " violating" : ""}`,
      transform: `translate(${round(box.x)}, ${round(box.y)})`, tabindex: index ? -1 : 0, role: "button", "aria-label": node.title });
    if (violating) group.append(svg("path", { d: cloudPath(-7, -7, box.width + 14, box.height + 14), class: "cloud" }));
    group.append(svg("rect", { width: box.width, height: box.height, class: "box" }));
    if (node.entry_kind === "architecture") group.append(svg("rect", { x: 3.5, y: 3.5, width: box.width - 7, height: box.height - 7, class: "box-inner" }));
    const [title, subtitle, facts] = nodeLines(node);
    group.append(svg("text", { x: 12, y: facts === null ? 30 : 24, class: "node-title" }, title));
    group.append(svg("text", { x: 12, y: facts === null ? 48 : 40, class: "node-subtitle" }, subtitle));
    if (facts) group.append(svg("text", { x: 12, y: 55, class: "node-facts" }, facts));
    // Share of files with any observed dependency: low coverage makes a clean
    // check weak evidence, so it is shown on every architecture card.
    if (hasValue(node.observed_file_count) && node.file_count > 0) {
      const track = box.width - 24;
      group.append(svg("rect", { x: 12, y: 60, width: track, height: 2, class: "coverage-track" }));
      group.append(svg("rect", { x: 12, y: 60, width: round(track * Math.min(1, node.observed_file_count / node.file_count)), height: 2, class: "coverage" }));
    }
    group.append(svg("title", {}, [node.title, node.architecture_id || node.file_path, node.description,
      hasValue(node.observed_file_count) ? `${node.observed_file_count} of ${node.file_count} files have an observed dependency` : null].filter(Boolean).join("\n")));
    group.addEventListener("click", () => showNode(node, group));
    group.addEventListener("dblclick", () => { if (node.entry_kind === "architecture") loadFocus(node.architecture_id); });
    group.addEventListener("mouseenter", () => paint(neighbourhood(node.id), "hover", "has-hover"));
    group.addEventListener("mouseleave", () => paint(null, "hover", "has-hover"));
    group.addEventListener("focus", () => paint(neighbourhood(node.id), "hover", "has-hover"));
    group.addEventListener("blur", () => paint(null, "hover", "has-hover"));
    group.addEventListener("keydown", (event) => nodeKey(event, node, group));
    graph.append(group);
    drawing.nodes.set(node.id, group);
  });
}
// Keyboard: the drawing is one tab stop; arrow keys move between entries,
// Enter or Space shows details, Shift+Enter opens an architecture node.
function nodeKey(event, node, element) {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    if (event.key === "Enter" && event.shiftKey && node.entry_kind === "architecture") loadFocus(node.architecture_id);
    else showNode(node, element);
    return;
  }
  const directions = { ArrowRight: [1, 0], ArrowLeft: [-1, 0], ArrowDown: [0, 1], ArrowUp: [0, -1] };
  let target = null;
  if (event.key === "Home" || event.key === "End") {
    const ids = [...drawing.nodes.keys()].sort((a, b) => {
      const p = drawing.positions.get(a), q = drawing.positions.get(b);
      return p.y - q.y || p.x - q.x;
    });
    target = event.key === "Home" ? ids[0] : ids[ids.length - 1];
  } else if (directions[event.key]) {
    target = nearest(node.id, directions[event.key]);
  } else return;
  event.preventDefault();
  if (target) moveTo(target);
}
function nearest(id, [dx, dy]) {
  const here = drawing.positions.get(id);
  let best = null, bestScore = Infinity;
  for (const [other, box] of drawing.positions) {
    const along = (box.x - here.x) * dx + (box.y - here.y) * dy;
    if (other === id || along <= 0) continue;
    const score = along + 2 * (Math.abs((box.x - here.x) * dy) + Math.abs((box.y - here.y) * dx));
    if (score < bestScore) { bestScore = score; best = other; }
  }
  return best;
}
function moveTo(id) {
  for (const element of drawing.nodes.values()) element.setAttribute("tabindex", "-1");
  const element = drawing.nodes.get(id);
  element.setAttribute("tabindex", "0");
  element.focus();
  element.scrollIntoView({ block: "nearest", inline: "nearest" });
}
function applyZoom() {
  if (!drawing) return;
  const available = $("graph-wrap").clientWidth;
  if (zoom.fit) zoom.scale = Math.min(1, Math.max(0.5, available / drawing.width));
  $("graph").setAttribute("width", Math.round(drawing.width * zoom.scale));
  $("graph").setAttribute("height", Math.round(drawing.height * zoom.scale));
  $("zoom-level").textContent = `${Math.round(zoom.scale * 100)}%`;
  $("zoom-fit").setAttribute("aria-pressed", String(zoom.fit));
}
function setZoom(factor) {
  zoom = factor ? { scale: Math.min(2.5, Math.max(0.3, zoom.scale * factor)), fit: false } : { scale: 1, fit: true };
  applyZoom();
}
async function loadFocus(id, pushHistory = true) {
  const request = ++focusRequest;
  $("loading").hidden = false;
  $("error").hidden = true;
  try {
    const projection = await api(`/api/focus/${encodeURIComponent(id)}`);
    if (request !== focusRequest) return;
    const sameFocus = currentProjection !== null && currentProjection.focus.id === projection.focus.id;
    currentProjection = projection;
    entries = new Map(projection.nodes.map((node) => [node.id, node]));
    merged = mergeEdges(projection.edges);
    selection = null;
    if (!sameFocus) table.filter = "";
    if (pushHistory) history.pushState({ focus: id }, "", focusUrl(id));
    const trail = html("ol");
    for (const [index, ancestor] of projection.breadcrumbs.entries()) {
      const link = linkTo(ancestor.id, ancestor.title);
      if (index === projection.breadcrumbs.length - 1) link.setAttribute("aria-current", "page");
      const item = html("li");
      item.append(link);
      trail.append(item);
    }
    $("breadcrumbs").replaceChildren(trail);
    const focus = projection.focus;
    $("focus-id").textContent = focus.id;
    $("focus-title").textContent = focus.title;
    $("description").textContent = focus.description || "No description authored for this node.";
    $("description").classList.toggle("muted", !focus.description);
    $("interfaces").replaceChildren();
    interfacesInto($("interfaces"), focus.interfaces);
    showSummary(projection);
    $("evidence-notice").textContent = projection.evidence_notice;
    renderView();
    $("violations-heading").textContent = `Violations in this view (${projection.violations.length})`;
    $("violations").replaceChildren();
    if (!projection.violations.length) $("violations").append(html("p", "No matching observed architecture violations.", "muted"));
    for (const violation of projection.violations) {
      const button = html("button", null, "violation-button");
      button.type = "button";
      button.append(html("span", violation.rule_id, "violation-rule"), html("span", violation.kind.replaceAll("_", " "), "violation-kind"),
        html("span", violation.message, "violation-message"));
      button.addEventListener("click", () => showViolation(violation, button));
      $("violations").append(button);
    }
    detailsTitle("Details & evidence").append(html("p", "Select a node, dependency, or violation.", "muted"));
    document.title = `${focus.title} · ArchGraph`;
  } catch (error) { if (request === focusRequest) showError(error); }
  finally { if (request === focusRequest) $("loading").hidden = true; }
}
// The title block: what this focus contains and how much of it GitNexus saw.
function showSummary(projection) {
  const focus = projection.focus;
  const inside = projection.nodes.filter((node) => !node.outside_focus).length;
  const outside = projection.nodes.length - inside;
  const share = focus.descendant_file_count ? Math.round(100 * focus.observed_file_count / focus.descendant_file_count) : 0;
  const cells = [
    ["Mapped files", focus.descendant_file_count, null],
    ["Observed", focus.observed_file_count, `${share}% of files`],
    ["Entries", inside, outside ? `and ${outside} outside` : "in this focus"],
    ["Dependencies", merged.length, `${projection.edges.length} by relation kind`],
    ["Violations", projection.violations.length, projection.violations.length ? "see below" : "none observed"],
  ];
  $("summary").replaceChildren();
  for (const [label, value, note] of cells) {
    const cell = html("div", null, `cell${label === "Violations" && value ? " alert" : ""}`);
    const data = html("dd");
    data.append(html("span", value, "value"));
    if (note) data.append(html("span", note, "sub"));
    if (label === "Observed") {
      const meter = svg("svg", { viewBox: "0 0 100 4", preserveAspectRatio: "none", class: "meter", "aria-hidden": "true" });
      meter.append(svg("rect", { width: 100, height: 4, class: "coverage-track" }), svg("rect", { width: share, height: 4, class: "coverage" }));
      data.append(meter);
    }
    cell.append(html("dt", label), data);
    $("summary").append(cell);
  }
}
function renderView() {
  if (!currentProjection) return;
  const count = currentProjection.nodes.length;
  const tooBig = count > DIAGRAM_LIMIT;
  view = tooBig ? "table" : viewChoice || "diagram";
  $("view-diagram").disabled = tooBig;
  $("view-diagram").title = tooBig ? `${count} entries are too many to draw. Open a smaller node for a diagram.` : "";
  $("view-diagram").setAttribute("aria-pressed", String(view === "diagram"));
  $("view-table").setAttribute("aria-pressed", String(view === "table"));
  for (const id of ["graph-wrap", "zoom"]) $(id).hidden = view !== "diagram";
  document.querySelector(".legend").hidden = view !== "diagram";
  $("table-wrap").hidden = view !== "table";
  $("sheet-hint").textContent = view === "diagram"
    ? "Click for details, double-click to open a node. Keys: arrows move, Enter shows details, Shift+Enter opens."
    : tooBig ? `${count} entries are too many to draw, so the table lists them. Filter it, or open a smaller node for a diagram.`
      : "Click a row for details, double-click to open a node.";
  if (view === "diagram") {
    $("table-wrap").replaceChildren();
    drawGraph(currentProjection);
    applyZoom();
  } else {
    drawing = null;
    $("graph").replaceChildren();
    drawTable(currentProjection);
  }
}
// Every entry of the level with its dependency counts: the only view of a
// level too large to draw, and a filterable index of the others.
const table = { filter: "", sort: "path", timer: null };
function tableGroup(node) {
  if (node.outside_focus) return "Outside this focus";
  if (node.entry_kind === "architecture") return "Child nodes";
  if (node.entry_kind === "file" && node.file_path) return splitPath(node.file_path)[0] || "Repository root";
  return "This node";
}
function drawTable(projection) {
  const filter = html("input");
  Object.assign(filter, { id: "table-filter", type: "search", placeholder: "Filter by path or title", autocomplete: "off", spellcheck: false, value: table.filter });
  const filterLabel = html("label", "Filter entries", "visually-hidden");
  filterLabel.htmlFor = "table-filter";
  const sort = html("select");
  sort.id = "table-sort";
  for (const [value, text] of [["path", "Group by directory"], ["links", "Most dependencies first"], ["rules", "Violations first"]]) {
    const option = html("option", text);
    option.value = value;
    sort.append(option);
  }
  sort.value = table.sort;
  const sortLabel = html("label", "Order", "visually-hidden");
  sortLabel.htmlFor = "table-sort";
  const count = html("p", "", "table-count");
  count.setAttribute("aria-live", "polite");
  const tools = html("div", null, "table-tools");
  tools.append(filterLabel, filter, sortLabel, sort, count);
  const element = html("table", null, "entries");
  element.append(html("caption", `Entries of ${projection.focus.title}`, "visually-hidden"));
  const headRow = html("tr");
  for (const [text, className] of [["Entry", "entry-col"], ["Depends on", "num"], ["Used by", "num"], ["Violations", "num"]]) {
    const cell = html("th", text, className);
    cell.scope = "col";
    headRow.append(cell);
  }
  const head = html("thead");
  head.append(headRow);
  const body = html("tbody");
  element.append(head, body);
  const scroller = html("div", null, "table-scroll");
  scroller.append(element);
  $("table-wrap").replaceChildren(tools, scroller);
  const stats = new Map(projection.nodes.map((node) => [node.id, { out: 0, in: 0 }]));
  for (const edge of merged) { stats.get(edge.from).out += edge.count; stats.get(edge.to).in += edge.count; }
  const links = (node) => stats.get(node.id).out + stats.get(node.id).in;
  const rank = (node) => ({ "Child nodes": 0, "This node": 1, "Outside this focus": 3 })[tableGroup(node)] ?? 2;
  const name = (node) => node.file_path || node.architecture_id || node.title;
  const render = () => {
    const query = table.filter.trim().toLowerCase();
    const grouped = table.sort === "path";
    const shown = projection.nodes.filter((node) => !query || [node.title, node.file_path, node.architecture_id].some((value) => value && value.toLowerCase().includes(query)));
    shown.sort((a, b) => (grouped ? rank(a) - rank(b) || tableGroup(a).localeCompare(tableGroup(b)) : 0)
      || (table.sort === "rules" ? b.violation_rule_ids.length - a.violation_rule_ids.length : 0)
      || (grouped ? 0 : links(b) - links(a)) || name(a).localeCompare(name(b)));
    body.replaceChildren();
    let group = null;
    for (const node of shown) {
      if (grouped && tableGroup(node) !== group) {
        group = tableGroup(node);
        const row = html("tr", null, "group-row");
        const cell = html("th", group);
        cell.colSpan = 4;
        cell.scope = "colgroup";
        row.append(cell);
        body.append(row);
      }
      body.append(tableRow(node, stats.get(node.id), grouped));
    }
    if (!shown.length) {
      const row = html("tr");
      const cell = html("td", query ? "No entries match this filter." : "No mapped files or dependencies at this focus.", "empty");
      cell.colSpan = 4;
      row.append(cell);
      body.append(row);
    }
    count.textContent = shown.length === projection.nodes.length ? `${shown.length} entries` : `${shown.length} of ${projection.nodes.length} entries`;
  };
  filter.addEventListener("input", () => { clearTimeout(table.timer); table.timer = setTimeout(() => { table.filter = filter.value; render(); }, 120); });
  sort.addEventListener("change", () => { table.sort = sort.value; render(); });
  render();
}
function tableRow(node, stat, grouped) {
  const row = html("tr", null, `entry-row${node.outside_focus ? " outside" : ""}${node.violation_rule_ids.length ? " violating" : ""}`);
  if ($("details").dataset.entry === node.id) row.classList.add("selected");
  const file = node.entry_kind === "file" && node.file_path;
  const [directory, base] = file ? splitPath(node.file_path) : ["", ""];
  const button = html("button", null, `entry-button ${node.entry_kind}`);
  button.type = "button";
  button.append(html("span", file ? base : node.title, "entry-title"));
  const sub = file ? (grouped ? "" : directory) : node.architecture_id || ENTRY_KINDS[node.entry_kind];
  if (sub) button.append(html("span", sub, "entry-sub"));
  button.addEventListener("keydown", (event) => {
    const buttons = [...document.querySelectorAll("#table-wrap .entry-button")];
    const index = buttons.indexOf(button);
    if (event.key === "ArrowDown" && buttons[index + 1]) { event.preventDefault(); buttons[index + 1].focus(); }
    if (event.key === "ArrowUp" && buttons[index - 1]) { event.preventDefault(); buttons[index - 1].focus(); }
    if (event.key === "Enter" && event.shiftKey && node.entry_kind === "architecture") { event.preventDefault(); loadFocus(node.architecture_id); }
  });
  const cell = html("td", null, "entry-cell");
  cell.append(button);
  row.append(cell, html("td", stat.out || "", "num"), html("td", stat.in || "", "num"),
    html("td", node.violation_rule_ids.length || "", "num rules"));
  row.addEventListener("click", () => { showNode(node, row); $("details").dataset.entry = node.id; });
  row.addEventListener("dblclick", () => { if (node.entry_kind === "architecture") loadFocus(node.architecture_id); });
  return row;
}
function closeSearch() {
  ++searchRequest;
  $("search-results").replaceChildren();
}
async function searchNodes() {
  const request = ++searchRequest;
  const query = $("search").value.trim();
  $("search-results").replaceChildren();
  if (!query) return;
  try {
    const results = await api(`/api/search?q=${encodeURIComponent(query)}`);
    if (request !== searchRequest) return;
    if (!results.length) $("search-results").append(html("p", "No matching architecture nodes.", "muted"));
    for (const node of results) {
      const button = html("button", null, "search-result");
      button.type = "button";
      button.setAttribute("aria-label", `${node.title} — ${node.id}`);
      button.append(html("span", node.title, "result-title"), html("span", node.id, "result-id"));
      button.addEventListener("click", () => {
        closeSearch();
        $("search").value = "";
        loadFocus(node.id);
      });
      $("search-results").append(button);
    }
  } catch (error) { if (request === searchRequest) showError(error); }
}
function moveInResults(event) {
  const buttons = [...$("search-results").querySelectorAll("button")];
  const index = buttons.indexOf(document.activeElement);
  if (event.key === "ArrowDown" && buttons.length) { event.preventDefault(); buttons[Math.min(buttons.length - 1, index + 1)].focus(); }
  if (event.key === "ArrowUp" && index >= 0) { event.preventDefault(); (index ? buttons[index - 1] : $("search")).focus(); }
  if (event.key === "Escape") { closeSearch(); $("search").focus(); }
}
$("search").addEventListener("input", () => { ++searchRequest; clearTimeout(searchTimer); searchTimer = setTimeout(searchNodes, 180); });
$("search-form").addEventListener("submit", (event) => { event.preventDefault(); clearTimeout(searchTimer); searchNodes(); });
$("search-form").addEventListener("keydown", moveInResults);
document.addEventListener("click", (event) => { if (!$("search-form").contains(event.target)) closeSearch(); });
document.addEventListener("keydown", (event) => {
  const typing = event.target.closest && event.target.closest("input, select, textarea, [contenteditable]");
  if (event.key === "/" && !typing && !event.ctrlKey && !event.metaKey && !event.altKey) { event.preventDefault(); $("search").focus(); }
});
$("view-diagram").addEventListener("click", () => { viewChoice = "diagram"; renderView(); });
$("view-table").addEventListener("click", () => { viewChoice = "table"; renderView(); });
$("zoom-in").addEventListener("click", () => setZoom(1.2));
$("zoom-out").addEventListener("click", () => setZoom(1 / 1.2));
$("zoom-fit").addEventListener("click", () => setZoom(null));
window.addEventListener("resize", () => { if (zoom.fit) applyZoom(); });
window.addEventListener("popstate", () => { if (meta) loadFocus(new URL(location.href).searchParams.get("focus") || meta.project.root, false); });
(async () => {
  try {
    meta = await api("/api/meta");
    showDiagnostics();
    showRefreshState(false);
    await loadFocus(new URL(location.href).searchParams.get("focus") || meta.project.root, false);
    if (meta.watching) setInterval(() => { if (!document.hidden) checkForUpdates(); }, 5000);
  } catch (error) { showError(error); }
})();
