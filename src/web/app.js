"use strict";

// No HTML from architecture/provider data is interpreted. All dynamic content
// uses textContent; SVG is built with createElementNS and constant attribute keys.
// Styling goes through classes: the server's CSP refuses inline style attributes.
const $ = (id) => document.getElementById(id);
const SVG_NS = "http://www.w3.org/2000/svg";
let currentProjection = null;
let meta = null;
let focusRequest = 0;
let searchRequest = 0;
let searchTimer = null;
let entries = new Map(); // entry id → projection entry or directory group
let merged = []; // mergeEdges(currentProjection.edges), unfiltered, file level
let scene = null; // what the canvas shows: see buildScene
let selection = null; // entries and edges lit by the current selection
let selected = null; // { kind, id } of the selection, restored after redraws
let viewChoice = null;
let view = "diagram";
let camera = { x: 0, y: 0, k: 1 };
let cameraAnimation = 0;
let spaceHeld = false;
let tree = { byId: new Map(), children: new Map(), roots: [], counts: new Map(), open: new Set(), error: null };
let allViolations = [];
let allPackages = []; // imported packages (provider.packages), for search
const narrow = window.matchMedia("(max-width: 900px)");
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
// Levels with more entries than this group their files by directory.
const CLUSTER_LIMIT = 120;
const CLUSTER_TARGET = 32;
// Above this many drawn edges, labels appear only on highlighted edges.
const LABEL_LIMIT = 40;
const CARD = { width: 236, height: 86 }, GAP_X = 44, GAP_Y = 128, PAD = 40, MAX_COLUMNS = 6;
const MIN_ZOOM = 0.08, MAX_ZOOM = 3;

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
// Browser storage is a convenience only: it can be missing or throw.
function stored(key, fallback) {
  try { const value = window.localStorage.getItem(key); return value === null ? fallback : JSON.parse(value); } catch { return fallback; }
}
function store(key, value) {
  try { if (value === null) window.localStorage.removeItem(key); else window.localStorage.setItem(key, JSON.stringify(value)); } catch { /* not persisted */ }
}
const storageKey = (kind, focusId) => `archgraph.${kind}.v1:${meta ? meta.project.name : ""}:${focusId || ""}`;
function plural(count, noun) { return `${count} ${noun}${count === 1 ? "" : "s"}`; }
function hasValue(value) { return value !== null && value !== undefined; }
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
const round = (value) => Math.round(value * 10) / 10;
const clamp = (value, low, high) => Math.min(high, Math.max(low, value));

// ---------------------------------------------------------------- status
function showDiagnostics() {
  const list = $("diagnostics");
  if (!list) return;
  list.replaceChildren();
  $("diagnostics-section").hidden = !meta.diagnostics.length;
  $("diagnostics-summary").textContent = `Coverage diagnostics (${meta.diagnostics.length})`;
  // A few notes are shown at once; many start folded.
  $("diagnostics-section").open = meta.diagnostics.length <= 3;
  for (const warning of meta.diagnostics) list.append(html("li", warning));
}
function showNotesButton() {
  const count = meta.diagnostics.length;
  $("notes-button").hidden = !count;
  $("notes-button").replaceChildren(html("span", count, "notes-count"), html("span", ` coverage note${count === 1 ? "" : "s"}`, "notes-label"));
  $("notes-button").setAttribute("aria-label", `${plural(count, "coverage note")}: open them in the details panel`);
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
  showNotesButton();
  await loadTree();
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

// ---------------------------------------------------------------- panels
// Desktop: the sidebar and the inspector are columns that can be collapsed.
// Narrow screens: both are drawers over the canvas, one at a time.
const panels = stored("archgraph.panels.v1", { sidebar: true, inspector: true });
let drawer = null;
function applyPanels() {
  const body = document.body;
  body.classList.toggle("sidebar-closed", !panels.sidebar);
  body.classList.toggle("inspector-closed", !panels.inspector);
  body.classList.toggle("drawer-sidebar", drawer === "sidebar");
  body.classList.toggle("drawer-inspector", drawer === "inspector");
  const sidebarShown = narrow.matches ? drawer === "sidebar" : panels.sidebar;
  const inspectorShown = narrow.matches ? drawer === "inspector" : panels.inspector;
  $("sidebar-toggle").setAttribute("aria-expanded", String(sidebarShown));
  $("inspector-toggle").setAttribute("aria-expanded", String(inspectorShown));
  $("scrim").hidden = !(narrow.matches && drawer);
}
function togglePanel(name, force) {
  if (narrow.matches) drawer = (force ?? drawer !== name) ? name : null;
  else { panels[name] = force ?? !panels[name]; store("archgraph.panels.v1", panels); }
  applyPanels();
}
// On narrow screens the details are a drawer; open it after the user picks
// something, or the click seems to do nothing.
function revealDetails() {
  if (narrow.matches) togglePanel("inspector", true);
}

// ---------------------------------------------------------------- node list
// The whole architecture as a tree, built from dotted node ids, with the
// number of violations in each subtree.
async function loadTree() {
  // Packages only feed search: without them the rest still works.
  const packages = api("/api/packages").then((payload) => payload.packages || [], () => []);
  try {
    const [nodes, payload] = await Promise.all([api("/api/nodes"), api("/api/violations")]);
    buildTree(nodes, payload.violations || []);
  } catch (error) {
    tree = { ...tree, error: error.message || String(error) };
  }
  allPackages = await packages;
  renderTree();
  renderViolationList();
}
function buildTree(nodes, violations) {
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const parents = new Map(), children = new Map(), roots = [];
  for (const node of nodes) {
    let parent = null;
    for (let cut = node.id.lastIndexOf("."); cut > 0 && !parent; cut = node.id.lastIndexOf(".", cut - 1)) {
      if (byId.has(node.id.slice(0, cut))) parent = node.id.slice(0, cut);
    }
    parents.set(node.id, parent);
    if (!parent) roots.push(node.id);
    else { if (!children.has(parent)) children.set(parent, []); children.get(parent).push(node.id); }
  }
  const counts = new Map();
  for (const violation of violations) {
    const touched = new Set();
    for (const id of violation.affected_nodes && violation.affected_nodes.length ? violation.affected_nodes : violation.nodes || []) {
      for (let at = byId.has(id) ? id : null; at; at = parents.get(at)) touched.add(at);
    }
    for (const id of touched) counts.set(id, (counts.get(id) || 0) + 1);
  }
  tree = { byId, parents, children, roots, counts, open: tree.open, picked: tree.picked, error: null };
  allViolations = violations;
}
function renderTree() {
  const list = $("tree");
  const focused = document.activeElement && list.contains(document.activeElement) ? document.activeElement.closest("[role='treeitem']") : null;
  list.replaceChildren();
  if (tree.error) { list.append(html("li", `Cannot load the node list: ${tree.error}`, "side-error")); return; }
  const only = $("tree-violations").checked;
  const focusId = currentProjection ? currentProjection.focus.id : null;
  const build = (id, level, parent) => {
    const node = tree.byId.get(id), count = tree.counts.get(id) || 0;
    if (only && !count) return;
    const kids = (tree.children.get(id) || []).filter((kid) => !only || tree.counts.get(kid));
    const open = only || tree.open.has(id);
    const item = html("li", null, "tree-item");
    item.setAttribute("role", "treeitem");
    item.setAttribute("aria-level", String(level));
    item.setAttribute("aria-selected", String(id === focusId));
    item.setAttribute("aria-label", `${node.title}${count ? `, ${plural(count, "violation")}` : ""}`);
    item.tabIndex = -1;
    item.dataset.id = id;
    if (kids.length) item.setAttribute("aria-expanded", String(open));
    const row = html("div", null, `tree-row${id === focusId ? " current" : ""}${id === tree.picked ? " picked" : ""}`);
    const twisty = html("span", null, `twisty${kids.length ? "" : " leaf"}`);
    twisty.setAttribute("aria-hidden", "true");
    twisty.addEventListener("click", (event) => { event.stopPropagation(); toggleTreeItem(id); });
    const title = html("span", node.title, "tree-title");
    title.title = `${node.title}\n${id}`;
    row.append(twisty, title);
    if (count) row.append(html("span", count, "count-badge"));
    const opener = html("button", "Open", "tree-open");
    opener.type = "button";
    opener.tabIndex = -1;
    opener.setAttribute("aria-hidden", "true");
    opener.addEventListener("click", (event) => { event.stopPropagation(); loadFocus(id); });
    row.append(opener);
    row.addEventListener("click", () => { focusTreeItem(item); goToNode(id); });
    row.addEventListener("dblclick", () => loadFocus(id));
    item.append(row);
    if (kids.length && open) {
      const group = html("ul");
      group.setAttribute("role", "group");
      for (const kid of kids) build(kid, level + 1, group);
      item.append(group);
    }
    parent.append(item);
  };
  for (const id of tree.roots) build(id, 1, list);
  if (!list.childElementCount) list.append(html("li", only ? "No node has a violation." : "No architecture nodes.", "side-empty"));
  const current = (focused && list.querySelector(`[data-id="${CSS.escape(focused.dataset.id)}"]`))
    || list.querySelector("[aria-selected='true']") || list.querySelector("[role='treeitem']");
  if (current) current.tabIndex = 0;
  if (focused && current) current.focus();
}
// Marks the architecture node selected on the canvas in the node list.
function pickInTree(id) {
  tree.picked = id;
  for (const row of $("tree").querySelectorAll(".tree-row.picked")) row.classList.remove("picked");
  const item = id && $("tree").querySelector(`[data-id="${CSS.escape(id)}"]`);
  if (item) item.firstElementChild.classList.add("picked");
}
function toggleTreeItem(id, force) {
  const open = force ?? !tree.open.has(id);
  if (open) tree.open.add(id); else tree.open.delete(id);
  renderTree();
}
function focusTreeItem(item) {
  for (const other of $("tree").querySelectorAll("[role='treeitem']")) other.tabIndex = -1;
  item.tabIndex = 0;
  item.focus();
}
$("tree").addEventListener("keydown", (event) => {
  const item = event.target.closest("[role='treeitem']");
  if (!item) return;
  const items = [...$("tree").querySelectorAll("[role='treeitem']")];
  const index = items.indexOf(item), id = item.dataset.id;
  const expanded = item.getAttribute("aria-expanded");
  if (event.key === "ArrowDown" && items[index + 1]) focusTreeItem(items[index + 1]);
  else if (event.key === "ArrowUp" && items[index - 1]) focusTreeItem(items[index - 1]);
  else if (event.key === "Home") focusTreeItem(items[0]);
  else if (event.key === "End") focusTreeItem(items[items.length - 1]);
  else if (event.key === "ArrowRight" && expanded === "false") toggleTreeItem(id, true);
  else if (event.key === "ArrowRight" && expanded === "true") focusTreeItem(item.querySelector("[role='treeitem']"));
  else if (event.key === "ArrowLeft" && expanded === "true") toggleTreeItem(id, false);
  else if (event.key === "ArrowLeft" && item.parentElement.closest("[role='treeitem']")) focusTreeItem(item.parentElement.closest("[role='treeitem']"));
  else if (event.key === "Enter") { if (event.shiftKey) loadFocus(id); else goToNode(id); }
  else return;
  event.preventDefault();
});
$("tree-violations").addEventListener("change", renderTree);
// Shows a node on the canvas: selected and centred in the level where it is
// an entry (its parent's), or opened when it has no parent.
async function goToNode(id) {
  if (narrow.matches) togglePanel("sidebar", false);
  const entryId = `node:${id}`;
  if (scene && scene.byId.has(entryId)) { selectEntry(entryId, { centre: true }); return; }
  if (currentProjection && currentProjection.focus.id === id) { showOverview(); if (view === "diagram") fitView(); return; }
  const parent = tree.parents.get(id);
  await loadFocus(parent || id, true, parent ? { select: entryId } : {});
}
const violationKey = (violation) => JSON.stringify([violation.rule_id, violation.kind, violation.from, violation.to, violation.nodes]);
// The level where a violation's nodes are entries together: their lowest
// common ancestor, or the parent of a single node.
function violationHome(violation) {
  const ids = (violation.nodes && violation.nodes.length ? violation.nodes : violation.affected_nodes || []).filter((id) => tree.byId.has(id));
  if (!ids.length) return violation.from && tree.byId.has(violation.from) ? violation.from : null;
  const chain = (id) => { const list = []; for (let at = id; at; at = tree.parents.get(at)) list.unshift(at); return list; };
  let common = chain(ids[0]);
  for (const id of ids.slice(1)) {
    const other = chain(id);
    let same = 0;
    while (same < common.length && same < other.length && common[same] === other[same]) same++;
    common = common.slice(0, same);
  }
  let home = common[common.length - 1] || null;
  if (home && ids.includes(home)) home = tree.parents.get(home) || home;
  return home;
}
function renderViolationList() {
  const list = $("violations");
  list.replaceChildren();
  $("violations-heading").textContent = `Violations (${allViolations.length})`;
  if (tree.error) return;
  if (!allViolations.length) list.append(html("p", "No observed architecture violations.", "side-empty"));
  const here = new Set((currentProjection ? currentProjection.violations : []).map(violationKey));
  for (const violation of allViolations) {
    const key = violationKey(violation);
    const home = violationHome(violation);
    const button = html("button", null, `violation-button${here.has(key) ? " here" : ""}`);
    button.type = "button";
    button.dataset.key = key;
    button.append(html("span", violation.rule_id, "violation-rule"), html("span", violation.kind.replaceAll("_", " "), "violation-kind"));
    button.append(html("span", here.has(key) ? "In this view" : home ? `In ${tree.byId.get(home).title}` : "", "violation-where"));
    button.append(html("span", violation.message, "violation-message"));
    button.addEventListener("click", () => openViolation(violation));
    list.append(button);
  }
}
async function openViolation(violation) {
  const key = violationKey(violation);
  const find = () => (currentProjection ? currentProjection.violations : []).find((other) => violationKey(other) === key);
  if (!find()) {
    const home = violationHome(violation);
    if (home && (!currentProjection || home !== currentProjection.focus.id)) await loadFocus(home);
  }
  if (narrow.matches) togglePanel("sidebar", false);
  showViolation(find() || violation);
}

// ---------------------------------------------------------------- inspector
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
  $("overview-button").hidden = false;
  if (kind) panel.append(html("p", kind, "detail-kind"));
  panel.append(html("h2", title));
  if (id) panel.append(html("p", id, "detail-id"));
  return panel;
}
// Nothing selected: the panel describes the level itself.
function showOverview() {
  select(null, null);
  selected = null;
  pickInTree(null);
  const projection = currentProjection;
  if (!projection) return;
  const focus = projection.focus;
  const panel = detailsTitle(focus.title, focus.id, "This level");
  $("overview-button").hidden = true;
  const description = html("p", focus.description || "No description authored for this node.", focus.description ? "" : "muted");
  description.id = "description";
  const interfaces = html("div", null, "interfaces");
  interfaces.id = "interfaces";
  interfacesInto(interfaces, focus.interfaces);
  panel.append(description, interfaces, summaryList(projection));
  if (focusUnused(focus)) usageNote(panel, "idle", "No observed code outside this node depends on it, and it declares no entry point. Possibly started by a tool, or dead code; declare its entry points in project.entry_points if it has any.", IDLE_CAVEAT);
  panel.append(html("h3", `Violations in this view (${projection.violations.length})`));
  if (!projection.violations.length) panel.append(html("p", "No matching observed architecture violations.", "muted"));
  const list = html("ul", null, "plain-list");
  for (const violation of projection.violations) {
    const button = html("button", null, "overview-violation");
    button.type = "button";
    button.append(html("span", violation.rule_id, "violation-rule"), html("span", violation.message, "violation-message"));
    button.addEventListener("click", () => showViolation(violation));
    const item = html("li");
    item.append(button);
    list.append(item);
  }
  panel.append(list);
  const notes = html("details", null, "notes");
  notes.id = "diagnostics-section";
  const summary = html("summary");
  summary.id = "diagnostics-summary";
  const warnings = html("ul");
  warnings.id = "diagnostics";
  notes.append(summary, warnings);
  panel.append(notes);
  panel.append(html("p", "Architecture is authored in architecture.yaml. The view reloads when the GitNexus index or the architecture file changes; reindex GitNexus after source edits.", "notice"));
  showDiagnostics();
}
function summaryList(projection) {
  const focus = projection.focus;
  const inside = projection.nodes.filter((node) => !node.outside_focus).length;
  const outside = projection.nodes.length - inside;
  const share = focus.descendant_file_count ? Math.round(100 * focus.observed_file_count / focus.descendant_file_count) : 0;
  const list = html("dl", null, "facts");
  list.id = "summary";
  const row = (label, value, note, extra) => {
    const item = html("div", null, `fact${label === "Violations" && value ? " alert" : ""}${label === "No observed users" && value ? " idle" : ""}`);
    const data = html("dd");
    data.append(html("span", value, "value"));
    if (note) data.append(html("span", note, "sub"));
    if (extra) data.append(extra);
    item.append(html("dt", label), data);
    list.append(item);
  };
  const meter = svg("svg", { viewBox: "0 0 100 4", preserveAspectRatio: "none", class: "meter", "aria-hidden": "true" });
  meter.append(svg("rect", { width: 100, height: 4, class: "coverage-track" }), svg("rect", { width: share, height: 4, class: "coverage" }));
  row("Mapped files", focus.descendant_file_count, null);
  if (focus.descendant_package_count) row("Packages", focus.descendant_package_count, "imported third-party packages");
  row("Observed", focus.observed_file_count, `${share}% of files have a dependency`, meter);
  row("Entries", inside, outside ? `and ${outside} outside` : null);
  row("Dependencies", merged.length, `${projection.edges.length} by relation kind`);
  row("Entry points", focus.entry_point_count || 0, "declared in architecture.yaml");
  row("No observed users", focus.no_observed_users_count || 0, "files; candidates, not proof");
  row("Violations", projection.violations.length, null);
  return list;
}
// `lit`: { nodes: Set of entry ids, edges: Set of edges } to keep at full
// strength while everything else on the canvas is dimmed.
function select(element, lit) {
  for (const previous of document.querySelectorAll("#graph .selected, #table-wrap .selected, #violations .selected, #details .selected")) previous.classList.remove("selected");
  if (element) element.classList.add("selected");
  selection = lit || null;
  paint(selection, "lit", "has-selection");
}
function paint(lit, className, rootClass) {
  if (!scene || !scene.nodeEls) return;
  $("graph").classList.toggle(rootClass, Boolean(lit));
  for (const [id, element] of scene.nodeEls) element.classList.toggle(className, Boolean(lit) && lit.nodes.has(id));
  for (const item of scene.edgeEls) {
    item.element.classList.toggle(className, Boolean(lit) && lit.edges.has(item.edge));
    const strong = item.element.classList.contains("lit") || item.element.classList.contains("hover");
    item.line.setAttribute("marker-end", strong ? item.litMarker : item.marker);
  }
}
// Hovering an entry or edge on the canvas marks the rows naming it in the
// details panel; hovering a row lights its edge and entry on the canvas.
function linkPanel(source) {
  for (const row of document.querySelectorAll("#details .dependency")) {
    row.classList.toggle("linked", Boolean(source) && (source.edge ? row.edge === source.edge : row.otherId === source.node));
  }
}
function hoverCanvas(lit, source) {
  paint(lit, "hover", "has-hover");
  linkPanel(lit ? source : null);
}
// A path may break after a slash, never inside a name.
function pathText(text, className) {
  const element = html("span", null, className);
  String(text).split(/(?<=\/)/).forEach((part, index) => {
    if (index) element.append(document.createElement("wbr"));
    element.append(part);
  });
  return element;
}
function sceneEdges(id) { return scene && scene.byId.has(id) ? scene.edges : merged; }
function neighbourhood(id) {
  const lit = { nodes: new Set([id]), edges: new Set() };
  for (const edge of sceneEdges(id)) {
    if (edge.from !== id && edge.to !== id) continue;
    lit.edges.add(edge);
    lit.nodes.add(edge.from);
    lit.nodes.add(edge.to);
  }
  return lit;
}
function addEvidence(parent, evidence, total, limit = Infinity) {
  const list = html("ol", null, "evidence");
  for (const item of (evidence || []).slice(0, limit)) {
    const row = html("li", null, "evidence-item");
    const target = item.to_file.startsWith("package:") ? `${packageName(item.to_file)} (package)` : item.to_file;
    row.append(html("div", `${item.from_file}\n→ ${target}`, "evidence-pair"));
    const line = html("div", null, "evidence-meta");
    line.append(html("span", item.kind, "tag"));
    if (hasValue(item.confidence)) line.append(html("span", `confidence ${item.confidence}`));
    if (item.reason) line.append(html("span", item.reason, "reason"));
    row.append(line);
    list.append(row);
  }
  if (list.childElementCount) parent.append(list);
  const count = Math.min((evidence || []).length, limit);
  if (count < total) parent.append(html("p", `Showing ${count} of ${total} observations, sorted deterministically. Use archgraph context with --evidence-limit for more.`, "notice"));
}
const ENTRY_KINDS = { architecture: "Architecture node", file: "File", direct_files: "Directly owned files", boundary: "Boundary of this focus", group: "Directory group", package: "Package" };
const ECOSYSTEMS = { python: "Python", npm: "npm" };
function ecosystemTitle(ecosystem) { return ECOSYSTEMS[ecosystem] || ecosystem; }
function entryKind(node) {
  const kind = node.package ? `${ecosystemTitle(node.package.ecosystem)} package`
    : node.node_kind === "external" ? "External architecture node" : ENTRY_KINDS[node.entry_kind] || node.entry_kind;
  return node.outside_focus ? `${kind}, outside this focus` : kind;
}
function entryTitle(node) {
  if (node.entry_kind === "file" && node.file_path) return splitPath(node.file_path)[1];
  return node.title;
}

// ---------------------------------------------------------------- usage
// Whether observed code uses a file: declared entry points (loaded by a tool
// or runtime) are told apart from files nothing observed depends on, which
// are dead-code candidates, never proof.
const USAGE_LABELS = { entry_point: "entry point", used: "used", no_observed_users: "no observed users", not_indexed: "not indexed" };
const IDLE_NOTE = "No observed code depends on this. Possibly an entry point for a tool, or dead code; declare it in project.entry_points if it is an entry point.";
const IDLE_CAVEAT = "Not proof: GitNexus misses autoloading, HTML script tags, dynamic imports and files loaded by name. Read the file and search for its name before removing it.";
// Files of an entry with the given usage: 1 or 0 for a file, a count for
// nodes, direct files and directory groups.
function usageCount(node, usage) {
  if (node.entry_kind === "file") return node.usage === usage ? 1 : 0;
  if (node.entry_kind === "group") return node.members.filter((member) => member.usage === usage).length;
  return (usage === "entry_point" ? node.entry_point_count : usage === "no_observed_users" ? node.no_observed_users_count : 0) || 0;
}
// An architecture node nothing outside uses, with no entry point: the same
// rule as CompiledNode::no_outside_users, for the focus itself.
function focusUnused(focus) {
  return Boolean(focus.parent) && focus.descendant_file_count > (focus.unindexed_file_count || 0)
    && !focus.entry_point_count && !focus.outside_user_count;
}
// Shown on the canvas as calm, not red: it is not a violation.
function isIdle(node) { return node.no_outside_users || usageCount(node, "no_observed_users") > 0; }
// A short status for cards, search results, the table and screen readers.
function usageLabel(node) {
  if (node.entry_kind === "file") return node.usage && node.usage !== "used" ? USAGE_LABELS[node.usage] : "";
  const parts = [];
  const entries = usageCount(node, "entry_point"), idle = usageCount(node, "no_observed_users");
  if (node.no_outside_users) parts.push("no observed use from outside");
  if (entries) parts.push(plural(entries, "entry point"));
  if (idle) parts.push(`${plural(idle, "file")} with no observed users`);
  return parts.join(", ");
}
function usageNote(panel, className, text, caveat) {
  const note = html("div", null, `usage-note ${className}`);
  note.append(html("p", text));
  if (caveat) note.append(html("p", caveat, "usage-caveat"));
  panel.append(note);
}
function fileUsageDetails(panel, node) {
  if (node.usage === "entry_point") usageNote(panel, "entry", "Entry point, declared in architecture.yaml (project.entry_points). A tool, runtime or test runner loads it by name, so it needs no observed user.");
  else if (node.usage === "no_observed_users") usageNote(panel, "idle", IDLE_NOTE, IDLE_CAVEAT);
  else if (node.usage === "not_indexed") usageNote(panel, "unknown", "Not in the code-graph index, so whether anything uses it is unknown.");
  else if (node.usage === "used" && !merged.some((edge) => edge.to === node.id)) {
    usageNote(panel, "used", "Its observed users are outside the architecture (excluded, unassigned or ambiguously mapped files), so none is drawn here.");
  }
}
// Entry points and files with no observed users under a node or a group.
function groupUsageDetails(panel, node) {
  const entries = usageCount(node, "entry_point"), idle = usageCount(node, "no_observed_users");
  if (node.no_outside_users) {
    usageNote(panel, "idle", "No observed code outside this node depends on it, and it declares no entry point. Possibly started by a tool, or dead code; declare its entry points in project.entry_points if it has any.", IDLE_CAVEAT);
  }
  if (!entries && !idle) return;
  const line = html("p", [entries ? `${plural(entries, "declared entry point")}` : null, idle ? `${plural(idle, "file")} with no observed users` : null].filter(Boolean).join(" · "), "usage-summary");
  panel.append(line);
}
// Opens a node with the "only files with no observed users" filter on.
async function showIdleFiles(id) {
  filters.idleOnly = true;
  store("archgraph.filters.v1", filters);
  await loadFocus(id);
}
function actionButton(text, action, className = "primary") {
  const button = html("button", text, className);
  button.type = "button";
  button.addEventListener("click", action);
  return button;
}
function showNode(node, element) {
  select(element, neighbourhood(node.id));
  selected = { kind: "node", id: node.id };
  pickInTree(node.entry_kind === "architecture" ? node.architecture_id : null);
  const panel = detailsTitle(entryTitle(node), node.file_path || (node.entry_kind === "group" ? `${node.directory || "."}/${node.direct ? "*" : ""}` : node.package ? node.id : node.architecture_id || node.id), entryKind(node));
  if (node.entry_kind !== "file" && node.entry_kind !== "package") {
    const observed = hasValue(node.observed_file_count) && node.file_count ? `, ${node.observed_file_count} with observed dependencies` : "";
    const packages = node.package_count ? plural(node.package_count, "imported package") : "";
    panel.append(html("p", node.file_count || !packages ? `${plural(node.file_count, "mapped file")}${observed}${packages ? `, ${packages}` : ""}` : packages));
  }
  if (node.description && !node.package) panel.append(html("p", node.description));
  if (node.entry_kind === "file") fileUsageDetails(panel, node);
  else if (!node.package) groupUsageDetails(panel, node);
  if ((node.interfaces || []).length) {
    panel.append(html("h3", "Interfaces"));
    const chips = html("div", null, "interfaces");
    interfacesInto(chips, node.interfaces);
    panel.append(chips);
  }
  const actions = html("div", null, "actions");
  if (node.entry_kind === "architecture") actions.append(actionButton("Open architecture node", () => loadFocus(node.architecture_id)));
  if (node.entry_kind === "architecture" && !node.outside_focus && usageCount(node, "no_observed_users")) {
    actions.append(actionButton("Show files with no observed users", () => showIdleFiles(node.architecture_id), "secondary"));
  }
  if (node.entry_kind === "group") actions.append(actionButton("Expand group", () => setGroupOpen(node, true)));
  const openGroup = scene && openGroupOf(node);
  if (openGroup) actions.append(actionButton(`Collapse ${openGroup.label}`, () => setGroupOpen(openGroup, false), "secondary"));
  if (scene && scene.byId.has(node.id) && view === "diagram") actions.append(actionButton("Centre on canvas", () => centreOn(node.id), "secondary"));
  if (actions.childElementCount) panel.append(actions);
  if (node.package) packageDetails(panel, node);
  if (node.entry_kind === "file") panel.append(html("p", "File identity only. Use GitNexus or your editor for source and symbol details.", "muted"));
  if (node.entry_kind === "direct_files") {
    panel.append(html("h3", "Directly owned files"));
    const files = html("ul", null, "plain-list");
    for (const file of currentProjection.focus.direct_files) files.append(html("li", file, "detail-id"));
    panel.append(files);
  }
  if (node.entry_kind === "group") {
    panel.append(html("h3", `Files (${node.members.length})`));
    const files = html("ul", null, "dependency-list");
    for (const member of node.members) {
      const button = html("button", null, `dependency${member.violation_rule_ids.length ? " violating" : ""}`);
      button.type = "button";
      button.append(html("span", member.file_path, "dependency-name"));
      const status = usageLabel(member);
      if (status) button.append(html("span", status, `dependency-count usage-${member.usage}`));
      if (member.violation_rule_ids.length) button.append(html("span", "⚠", "dependency-count"));
      button.addEventListener("click", () => revealEntry(member.id));
      const item = html("li");
      item.append(button);
      files.append(item);
    }
    panel.append(files);
  }
  if (node.violation_rule_ids.length) {
    panel.append(html("h3", "Rules with violations"));
    for (const id of node.violation_rule_ids) panel.append(html("p", id, "violation-badge"));
  }
  const edges = sceneEdges(node.id);
  dependencyList(panel, "Depends on", edges.filter((edge) => edge.from === node.id), (edge) => edge.to);
  dependencyList(panel, "Used by", edges.filter((edge) => edge.to === node.id), (edge) => edge.from);
  revealDetails();
}
// Who imports a package: every file and line, grouped by the node owning the
// file, each node a link to its place in the architecture.
function packageDetails(panel, node) {
  const imports = node.package.imports || [];
  const byNode = new Map();
  for (const item of imports) {
    const owner = item.node || "";
    if (!byNode.has(owner)) byNode.set(owner, []);
    byNode.get(owner).push(item);
  }
  const files = new Set(imports.map((item) => item.file));
  const owner = node.package.node;
  const ownerName = !owner ? "no node (ambiguous mapping)" : tree.byId && tree.byId.has(owner) ? `${tree.byId.get(owner).title} (${owner})` : owner;
  panel.append(html("p", `Imported by ${plural(files.size, "file")} in ${plural([...byNode.keys()].filter(Boolean).length, "node")}. Owned by ${ownerName}.`, "package-summary"));
  panel.append(html("h3", `Imports (${imports.length})`));
  // One fold per importing node; small ones start open, big ones folded.
  const sorted = [...byNode].sort((a, b) => a[0].localeCompare(b[0]));
  const groups = html("div", null, "importers");
  for (const [importer, items] of sorted) {
    const known = importer && tree.byId && tree.byId.has(importer);
    const group = html("details", null, "importer-group");
    group.open = sorted.length === 1 || items.length <= 12;
    const summary = html("summary", null, "importer-node");
    summary.append(html("span", known ? tree.byId.get(importer).title : importer || "Unassigned files", "importer-title"),
      html("span", `${importer ? `${importer} · ` : ""}${plural(items.length, "import")}`, "detail-id"));
    group.append(summary);
    if (known) {
      const link = html("button", "Show this node", "text-button");
      link.type = "button";
      link.addEventListener("click", () => goToNode(importer));
      group.append(link);
    }
    const lines = html("ul", null, "plain-list importer-lines");
    for (const item of items) {
      const line = html("li", null, "importer-line");
      line.append(html("span", `${item.file}:${item.line}`, "detail-id"));
      line.append(html("span", `${item.specifier}${item.type_only ? " (type only)" : ""}`, "importer-specifier"));
      lines.append(line);
    }
    group.append(lines);
    groups.append(group);
  }
  panel.append(groups);
}
// Each dependency of the selected entry, as a button that opens its evidence:
// the keyboard route to edges, which are not tab stops on the canvas.
function dependencyList(panel, heading, edges, other) {
  if (!edges.length) return;
  const limit = 200;
  panel.append(html("h3", `${heading} (${edges.length})`));
  const list = html("ul", null, "dependency-list");
  const sorted = [...edges].sort((a, b) => b.count - a.count || displayEndpoint(other(a)).localeCompare(displayEndpoint(other(b))));
  for (const edge of sorted.slice(0, limit)) {
    const button = html("button", null, `dependency${edge.violation_rule_ids.length ? " violating" : ""}`);
    button.type = "button";
    // The wire's colour, as drawn on the canvas.
    if (scene && scene.colours && scene.colours.mode !== "none") {
      button.append(wireSample(edgeLook(edge), { violating: edge.violation_rule_ids.length > 0, manual: edge.origin === "manual", cut: edge.suggested_cut_rule_ids.length > 0 }));
    }
    button.append(pathText(displayEndpoint(other(edge)), "dependency-name"));
    button.edge = edge;
    button.otherId = other(edge);
    const light = () => hoverCanvas({ nodes: new Set([edge.from, edge.to]), edges: new Set([edge]) }, { edge });
    const unlight = () => hoverCanvas(null);
    button.addEventListener("mouseenter", light);
    button.addEventListener("mouseleave", unlight);
    button.addEventListener("focus", light);
    button.addEventListener("blur", unlight);
    button.append(html("span", `${edgeSummary(edge)}${edge.origin === "manual" ? " manual" : ""}${edge.violation_rule_ids.length ? " ⚠" : ""}`, "dependency-count"));
    button.addEventListener("click", () => {
      const drawn = scene && scene.edgeEls && scene.edgeEls.find((item) => item.edge === edge);
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
  if (!node) return id;
  if (node.entry_kind === "group") return `${node.directory || "."}/${node.direct ? "*" : ""}`;
  if (node.package) return node.id;
  return node.file_path || node.architecture_id || node.title;
}
// One drawn edge per endpoint pair and origin. Relation kinds (CALLS, IMPORTS,
// ...) between the same two nodes are listed in its details instead of being
// drawn as parallel arrows with overlapping labels.
function mergeEdges(edges) {
  const groups = new Map();
  for (const edge of edges) {
    const key = [edge.from, edge.to, edge.origin].join("\u0000");
    if (!groups.has(key)) groups.set(key, { from: edge.from, to: edge.to, origin: edge.origin, count: 0, parts: [] });
    const group = groups.get(key);
    group.count += edge.count;
    group.parts.push(edge);
  }
  const union = (parts, field) => [...new Set(parts.flatMap((part) => part[field] || []))].sort();
  return [...groups.values()].map((group) => ({ ...group,
    violation_rule_ids: union(group.parts, "violation_rule_ids"),
    suggested_cut_rule_ids: union(group.parts, "suggested_cut_rule_ids") }));
}
function kindCounts(parts) {
  const counts = new Map();
  for (const part of parts) counts.set(part.kind, (counts.get(part.kind) || 0) + part.count);
  return [...counts].sort((a, b) => a[0].localeCompare(b[0]));
}
function edgeSummary(group) {
  const kinds = kindCounts(group.parts);
  return kinds.length === 1 ? `${compact(kinds[0][0], 22)} × ${group.count}` : `${kinds.length} kinds × ${group.count}`;
}
function showEdge(group, element) {
  select(element, { nodes: new Set([group.from, group.to]), edges: new Set([group]) });
  selected = { kind: "edge", from: group.from, to: group.to, origin: group.origin };
  const packageIds = edgePackages(group);
  const onlyPackages = packageIds.length > 0 && group.parts.every((edge) => (edge.evidence || []).every((item) => item.to_file.startsWith("package:")));
  // An edge into packages is titled by what it imports, not by "IMPORTS × n".
  const title = onlyPackages ? (packageIds.length <= 3 ? packageIds.map(packageName).join(", ") : plural(packageIds.length, "package")) : edgeSummary(group);
  const panel = detailsTitle(title, null, onlyPackages ? `Imported packages · ${edgeSummary(group)}` : group.origin === "manual" ? "Manual dependency" : "Observed dependency");
  panel.append(html("p", `${displayEndpoint(group.from)}\n→ ${displayEndpoint(group.to)}`, "evidence-pair endpoints"));
  panel.append(html("p", group.origin === "manual" ? "Manual relationship: descriptive intent, not source evidence."
    : onlyPackages ? "Imports of third-party packages, read from the source by ArchGraph." : "Observed file dependencies from GitNexus."));
  if (packageIds.length) packageSection(panel, packageIds, group);
  if (group.violation_rule_ids.length) panel.append(html("p", `⚠ ${group.violation_rule_ids.join(", ")}`, "violation-badge"));
  if (group.suggested_cut_rule_ids.length) panel.append(html("p", `✂ Suggested cut for ${group.suggested_cut_rule_ids.join(", ")}: removing this upward dependency helps break the cycle at the lowest observed cost.`, "cut-badge"));
  if (group.grouped) {
    // Between directory groups: totals per kind, then the file-level evidence.
    panel.append(html("h3", "Relation kinds"));
    for (const [kind, count] of kindCounts(group.parts)) panel.append(html("p", `${kind} × ${count}`, "detail-id"));
    panel.append(html("h3", `File dependencies (${group.parts.length})`));
    let shown = 0;
    for (const part of group.parts) {
      if (shown >= 100) break;
      addEvidence(panel, part.evidence, 0, 100 - shown);
      shown += Math.min((part.evidence || []).length, 100 - shown);
    }
    if (group.parts.length && shown >= 100) panel.append(html("p", "Showing the first 100 observations. Expand the groups to see single files.", "notice"));
  } else {
    for (const edge of group.parts) {
      panel.append(html("h3", `${edge.kind} × ${edge.count}`));
      if (hasValue(edge.confidence_min)) panel.append(html("p", `Confidence range: ${edge.confidence_min}–${edge.confidence_max}`));
      for (const manual of edge.manual_edges || []) {
        panel.append(html("p", manual.label || manual.id, "detail-id"));
        panel.append(html("p", `${manual.from} → ${manual.to}`, "detail-id"));
        if (manual.description) panel.append(html("p", manual.description));
      }
      if (edge.origin === "observed") addEvidence(panel, edge.evidence, edge.count);
    }
  }
  revealDetails();
}
// Packages an edge reaches, in the order of their names.
function edgePackages(group) {
  const ids = new Set();
  for (const edge of group.parts) for (const item of edge.evidence || []) if (item.to_file.startsWith("package:")) ids.add(item.to_file);
  return [...ids].sort((a, b) => packageName(a).localeCompare(packageName(b)));
}
function packageName(id) {
  const known = allPackages.find((item) => item.id === id);
  return known ? known.name : id.replace(/^package:[^/]+\//, "");
}
// An edge into packages names each package prominently, with the lines that
// import it from this edge's files; the name opens the package.
function packageSection(panel, ids, group) {
  const sources = new Set();
  for (const edge of group.parts) for (const item of edge.evidence || []) sources.add(item.from_file);
  panel.append(html("h3", `Packages (${ids.length})`));
  const list = html("ul", null, "edge-packages");
  for (const id of ids) {
    const known = allPackages.find((item) => item.id === id);
    const item = html("li", null, "edge-package");
    const name = html("button", packageName(id), "edge-package-name");
    name.type = "button";
    name.title = `Open ${id}`;
    if (known && known.node) name.addEventListener("click", () => openPackage(known));
    else name.disabled = true;
    item.append(name, html("div", known ? `${ecosystemTitle(known.ecosystem)} package` : id, "edge-package-kind"));
    const lines = html("ul", null, "importer-lines");
    for (const use of (known && known.imports) || []) {
      if (!sources.has(use.file)) continue;
      const row = html("li", null, "importer-line");
      row.append(html("span", `${use.file}:${use.line}`, "detail-id"), html("span", `${use.specifier}${use.type_only ? " (type only)" : ""}`, "detail-id"));
      lines.append(row);
    }
    if (lines.childElementCount) item.append(lines);
    list.append(item);
  }
  panel.append(list);
}
// Lights the entries and drawn edges this violation involves.
function violationScope(violation) {
  const lit = { nodes: new Set(), edges: new Set() };
  const affected = new Set(violation.affected_nodes || violation.nodes || []);
  for (const node of scene ? scene.entries : currentProjection.nodes) {
    if ((node.entry_kind === "architecture" && affected.has(node.architecture_id)) || (node.entry_kind === "group" && node.violation_rule_ids.includes(violation.rule_id))) lit.nodes.add(node.id);
  }
  for (const edge of scene ? scene.edges : merged) {
    if (!edge.violation_rule_ids.includes(violation.rule_id)) continue;
    lit.edges.add(edge);
    lit.nodes.add(edge.from);
    lit.nodes.add(edge.to);
  }
  return lit;
}
function showViolation(violation) {
  const key = violationKey(violation);
  const button = [...document.querySelectorAll("#violations .violation-button")].find((item) => item.dataset.key === key);
  select(button || null, violationScope(violation));
  selected = { kind: "violation", violation };
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
// After a redraw (filters, groups, reload) the same thing stays selected.
function restoreSelection() {
  if (!selected || !scene || !scene.nodeEls) return;
  if (selected.kind === "node") {
    const element = scene.nodeEls.get(selected.id);
    if (element) select(element, neighbourhood(selected.id));
    else if (!scene.byId.has(selected.id) && !entries.has(selected.id)) showOverview();
    else if (!scene.byId.has(selected.id) && entries.get(selected.id).entry_kind === "group") showOverview();
  } else if (selected.kind === "edge") {
    const item = scene.edgeEls.find(({ edge }) => edge.from === selected.from && edge.to === selected.to && edge.origin === selected.origin);
    if (item) select(item.element, { nodes: new Set([item.edge.from, item.edge.to]), edges: new Set([item.edge]) });
  } else if (selected.kind === "violation") {
    select(null, violationScope(selected.violation));
  }
}

// ---------------------------------------------------------------- scene
// Filters apply to what the canvas draws; the layout ignores the kind and
// origin filters so that entries do not jump when a filter changes.
const filters = Object.assign({ violationsOnly: false, idleOnly: false, outside: true, observed: true, manual: true, hiddenKinds: [] }, stored("archgraph.filters.v1", {}));
function edgeShown(edge) {
  return (edge.origin === "manual" ? filters.manual : filters.observed) && !filters.hiddenKinds.includes(edge.kind);
}
// Levels too large to draw file by file show their files as directory groups
// that expand in place; an expanded group is replaced by its subdirectories
// and files.
function clusterFiles(files, open) {
  const cache = new Map();
  const group = (prefix, members, direct) => ({ key: direct ? `${prefix}/*` : prefix, prefix, direct, files: members });
  const split = (item) => {
    if (cache.has(item.key)) return cache.get(item.key);
    let parts;
    if (item.direct) parts = item.files.map((file) => ({ file }));
    else {
      const base = item.prefix ? `${item.prefix}/` : "";
      const subs = new Map(), direct = [];
      for (const file of item.files) {
        const rest = file.file_path.slice(base.length), slash = rest.indexOf("/");
        if (slash < 0) { direct.push(file); continue; }
        const sub = base + rest.slice(0, slash);
        if (!subs.has(sub)) subs.set(sub, []);
        subs.get(sub).push(file);
      }
      parts = [...subs.keys()].sort().map((sub) => subs.get(sub).length === 1 ? { file: subs.get(sub)[0] } : group(sub, subs.get(sub), false));
      if (direct.length) parts.push(...(direct.length <= 3 ? direct.map((file) => ({ file })) : [group(item.prefix, direct, true)]));
      // A single subdirectory and nothing else: look inside it instead.
      if (parts.length === 1 && parts[0].files && !parts[0].direct) parts = split(parts[0]);
    }
    cache.set(item.key, parts);
    return parts;
  };
  const directories = files.map((file) => splitPath(file.file_path)[0].split("/"));
  let common = directories[0];
  for (const directory of directories) {
    let same = 0;
    while (same < common.length && same < directory.length && common[same] === directory[same]) same++;
    common = common.slice(0, same);
  }
  const order = (item) => item.files ? item.key : item.file.file_path;
  let items = split(group(common.join("/"), files, false));
  const opened = new Set();
  for (;;) {
    let changed = true;
    while (changed) {
      changed = false;
      items = items.flatMap((item) => {
        if (!item.files || !open.has(item.key) || opened.has(item.key)) return [item];
        opened.add(item.key);
        changed = true;
        return split(item);
      });
    }
    // Split the biggest directory while the canvas stays within the target.
    let best = null;
    for (const item of items) {
      if (!item.files || item.direct) continue;
      const parts = split(item);
      if (parts.length > 1 && items.length - 1 + parts.length <= CLUSTER_TARGET && (!best || item.files.length > best.files.length)) best = item;
    }
    if (!best) break;
    items = items.flatMap((item) => item === best ? split(item) : [item]);
  }
  items.sort((a, b) => order(a).localeCompare(order(b)));
  const groups = [], owner = new Map();
  for (const item of items) {
    if (!item.files) continue;
    const id = `group:${item.key}`;
    const last = splitPath(item.prefix)[1] || item.prefix || ".";
    groups.push({ id, key: item.key, title: item.direct ? `${last}/*` : `${last}/`, entry_kind: "group", architecture_id: null, node_kind: null,
      file_path: null, directory: item.prefix, direct: item.direct, file_count: item.files.length, observed_file_count: null,
      description: item.direct ? `Files directly in ${item.prefix || "the repository root"}/` : null, interfaces: [], outside_focus: false,
      members: item.files, violation_rule_ids: [...new Set(item.files.flatMap((file) => file.violation_rule_ids))].sort() });
    for (const file of item.files) owner.set(file.id, id);
  }
  return { groups, owner };
}
function buildScene(projection) {
  const focusId = projection.focus.id;
  const open = new Set(stored(storageKey("groups", focusId), []));
  const inside = projection.nodes.filter((node) => !node.outside_focus);
  const files = inside.filter((node) => node.entry_kind === "file" && node.file_path);
  const clustered = inside.length > CLUSTER_LIMIT && files.length > 1;
  const { groups, owner } = clustered ? clusterFiles(files, open) : { groups: [], owner: new Map() };
  const groupById = new Map(groups.map((item) => [item.id, item]));
  const list = [], placed = new Set();
  for (const node of projection.nodes) {
    if (node.outside_focus && !filters.outside) continue;
    const groupId = owner.get(node.id);
    if (!groupId) list.push(node);
    else if (!placed.has(groupId)) { placed.add(groupId); list.push(groupById.get(groupId)); }
  }
  const byId = new Map(list.map((node) => [node.id, node]));
  const project = (edges) => mergeEdges(edges.map((edge) => ({ ...edge, from: owner.get(edge.from) || edge.from, to: owner.get(edge.to) || edge.to }))
    .filter((edge) => edge.from !== edge.to && byId.has(edge.from) && byId.has(edge.to)))
    .map((edge) => ({ ...edge, grouped: groupById.has(edge.from) || groupById.has(edge.to) }));
  for (const item of groups) entries.set(item.id, item);
  return { focusId, clustered, open, groups, owner, entries: list, byId, layoutEdges: project(projection.edges),
    edges: project(projection.edges.filter(edgeShown)), layers: clustered ? null : projection.layers };
}
// The deepest group the user expanded that contains this entry, so that it
// can be collapsed again from the entry's details.
function openGroupOf(node) {
  if (!scene || !scene.clustered) return null;
  const path = node.entry_kind === "group" ? (node.direct ? `${node.directory}/*` : node.directory) : node.file_path;
  if (!path) return null;
  let best = null;
  for (const key of scene.open) {
    const direct = key.endsWith("/*"), prefix = direct ? key.slice(0, -2) : key;
    const inside = direct ? node.entry_kind === "file" && splitPath(path)[0] === prefix : prefix === "" || path.startsWith(`${prefix}/`);
    if (inside && (!best || key.length > best.key.length)) best = { key, label: direct ? `${prefix}/*` : `${prefix}/` };
  }
  return best;
}
function setGroupOpen(target, open) {
  if (!scene) return;
  const keys = new Set(scene.open);
  if (open) keys.add(target.key);
  else if (target.key.endsWith("/*")) keys.delete(target.key);
  else for (const key of [...keys]) if (key === target.key || key.startsWith(`${target.key}/`)) keys.delete(key);
  store(storageKey("groups", scene.focusId), [...keys].sort());
  const before = new Map([...scene.positions].map(([id, box]) => [id, { ...box }]));
  const anchor = open ? before.get(`group:${target.key}`) : null;
  redraw();
  settle(before, anchor);
}
// Shows an entry of this level on the canvas, opening the groups it is in.
function revealEntry(id) {
  if (view !== "diagram") setView("diagram");
  for (let guard = 0; guard < 50 && !scene.byId.has(id) && scene.owner.has(id); guard++) {
    const keys = new Set(scene.open);
    keys.add(scene.byId.get(scene.owner.get(id)).key);
    store(storageKey("groups", scene.focusId), [...keys].sort());
    redraw();
  }
  if (scene.byId.has(id)) selectEntry(id, { centre: true });
}
function selectEntry(id, { centre = false } = {}) {
  const node = scene && scene.byId.get(id);
  if (!node) return;
  showNode(node, scene.nodeEls ? scene.nodeEls.get(id) : null);
  if (centre && view === "diagram") centreOn(id);
}

// ---------------------------------------------------------------- layout
// Layers for levels the server did not layer (more than 60 entries, or
// directory groups): cycles are broken by a depth-first search in entry
// order, then each entry sits one row below its lowest dependent. Entries
// without any edge go to a last row of their own.
function clientLayers(ids, edges) {
  const out = new Map(ids.map((id) => [id, []]));
  const linked = new Set();
  for (const edge of edges) {
    if (!out.has(edge.from) || !out.has(edge.to) || edge.from === edge.to) continue;
    out.get(edge.from).push(edge.to);
    linked.add(edge.from);
    linked.add(edge.to);
  }
  const state = new Map(), finished = [], kept = new Map(ids.map((id) => [id, []]));
  const visit = (id) => {
    state.set(id, 1);
    for (const next of out.get(id)) {
      if (state.get(next) === 1) continue;
      kept.get(id).push(next);
      if (!state.has(next)) visit(next);
    }
    state.set(id, 2);
    finished.push(id);
  };
  for (const id of ids) if (linked.has(id) && !state.has(id)) visit(id);
  const level = new Map();
  for (const id of finished.reverse()) {
    if (!level.has(id)) level.set(id, 0);
    for (const next of kept.get(id)) level.set(next, Math.max(level.get(next) || 0, level.get(id) + 1));
  }
  const layers = [];
  for (const id of ids) if (level.has(id)) (layers[level.get(id)] ||= []).push(id);
  const loose = ids.filter((id) => !linked.has(id));
  return [...layers.filter(Boolean), ...(loose.length ? [loose] : [])];
}
// Rows from upper to lower layer, so dependencies mostly point down. Outside
// entries that only depend on this focus get a band above the focus frame;
// all other outside entries a band below. Within a row, entries are ordered
// by the mean position of their neighbours (deterministic barycentre sweeps).
// Every display mode starts from this arrangement.
function arrange(list, edges, serverLayers) {
  const chunk = (ids, size) => { const rows = []; for (let i = 0; i < ids.length; i += size) rows.push(ids.slice(i, i + size)); return rows; };
  const inside = list.filter((node) => !node.outside_focus).map((node) => node.id);
  const outside = list.filter((node) => node.outside_focus);
  const callers = outside.filter((node) => !edges.some((edge) => edge.to === node.id)).map((node) => node.id);
  const others = outside.map((node) => node.id).filter((id) => !callers.includes(id));
  const layers = serverLayers && serverLayers.length ? serverLayers : clientLayers(inside, edges);
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
  return { rows, centre, innerWidth, rowWidth, order: rows.flatMap((row) => row.ids) };
}
// Curves mode: the arrangement's rows, centred, at a fixed gap.
function layout({ rows, innerWidth, rowWidth }) {
  const left = PAD + 24;
  const positions = new Map(), notes = [];
  let y = PAD, bottom = PAD, band = null;
  rows.forEach((row, rowIndex) => {
    if (row.band !== band) {
      if (band) y = bottom + (band === "inside" ? 96 : 72);
      band = row.band;
      if (band === "inside") y += 52;
      else { notes.push({ y: y + 14, band }); y += 32; }
    }
    const offset = left + (innerWidth - rowWidth(row.ids.length)) / 2;
    row.ids.forEach((id, index) => positions.set(id, { x: offset + index * (CARD.width + GAP_X), y, width: CARD.width, height: CARD.height, row: rowIndex }));
    bottom = y + CARD.height;
    y += CARD.height + GAP_Y;
  });
  return { positions, notes, left };
}
// The focus frame follows its entries, also while they are dragged. On a
// board it also takes in the traces between inside entries, which run in
// channels around the cards.
function frameBox() {
  let box = null;
  const add = (x1, y1, x2, y2) => {
    box = box ? { x1: Math.min(box.x1, x1), y1: Math.min(box.y1, y1), x2: Math.max(box.x2, x2), y2: Math.max(box.y2, y2) } : { x1, y1, x2, y2 };
  };
  for (const node of scene.entries) {
    if (node.outside_focus) continue;
    const p = scene.positions.get(node.id), ear = p.hex || 0;
    add(p.x - ear, p.y, p.x + p.width + ear, p.y + p.height);
  }
  if (box) for (const point of scene.frameTraces || []) add(point.x, point.y, point.x, point.y);
  return box && { x: box.x1 - 28, y: box.y1 - 52, width: box.x2 - box.x1 + 56, height: box.y2 - box.y1 + 80 };
}
function contentBounds() {
  let box = null;
  const add = (b) => {
    box = box ? { x1: Math.min(box.x1, b.x), y1: Math.min(box.y1, b.y), x2: Math.max(box.x2, b.x + b.width), y2: Math.max(box.y2, b.y + b.height) }
      : { x1: b.x, y1: b.y, x2: b.x + b.width, y2: b.y + b.height };
  };
  for (const box2 of scene.positions.values()) add({ ...box2, x: box2.x - (box2.hex || 0), width: box2.width + 2 * (box2.hex || 0) });
  if (scene.board) { const e = scene.board.extent; add({ x: e.x1, y: e.y1, width: e.x2 - e.x1, height: e.y2 - e.y1 }); }
  const frame = frameBox();
  if (frame) add(frame);
  if (!box) return { x: 0, y: 0, width: 600, height: 400 };
  return { x: box.x1, y: box.y1, width: box.x2 - box.x1, height: box.y2 - box.y1 };
}

// ---------------------------------------------------------------- routing
// Downward edges leave a card's bottom and enter the next card's top; upward
// ones (against the layer order) run top to bottom and bow to the side, so
// they stand out. Each card side spreads its edges over several ports.
// Edges of entries the user moved take a direct curve instead.
function route(edges, positions, moved) {
  const plans = edges.map((edge, order) => {
    const a = positions.get(edge.from), b = positions.get(edge.to);
    if (!a || !b) throw new Error("Projection edge references an absent visible node");
    return { edge, order, a, b, free: moved.has(edge.from) || moved.has(edge.to), dir: b.row > a.row ? "down" : b.row < a.row ? "up" : "same" };
  });
  const ports = new Map();
  const attach = (id, box, side, plan, end, other) => {
    const key = `${id}\u0000${side}`;
    if (!ports.has(key)) ports.set(key, { box, side, items: [] });
    ports.get(key).items.push({ plan, end, other });
  };
  for (const plan of plans) {
    if (plan.free) continue;
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
  // between two cards (or beside the row) nearest to its straight course.
  const rows = new Map();
  for (const [id, box] of positions) {
    if (moved.has(id)) continue;
    if (!rows.has(box.row)) rows.set(box.row, []);
    rows.get(box.row).push(box);
  }
  for (const list of rows.values()) list.sort((p, q) => p.x - q.x);
  const channelUse = new Map();
  const channel = (row, x) => {
    const boxes = rows.get(row);
    if (!boxes) return x;
    const gaps = [boxes[0].x - GAP_X / 2, ...boxes.map((box) => box.x + box.width + GAP_X / 2)];
    let best = 0;
    gaps.forEach((gap, index) => { if (Math.abs(gap - x) < Math.abs(gaps[best] - x)) best = index; });
    const key = `${row}:${best}`, used = channelUse.get(key) || 0;
    channelUse.set(key, used + 1);
    return gaps[best] + ((used % 5) - 2) * 5;
  };
  for (const plan of plans) {
    if (plan.free) { freeRoute(plan); continue; }
    const s = plan.start, e = plan.end;
    plan.curves = [];
    if (plan.dir === "same") {
      const lift = 34 + Math.abs(e.x - s.x) * 0.14;
      plan.curves.push([s, { x: s.x, y: s.y - lift }, { x: e.x, y: e.y - lift }, e]);
      plan.d = `M ${round(s.x)} ${round(s.y)} ${curveTo(plan.curves[0])}`;
      continue;
    }
    const sign = plan.dir === "down" ? 1 : -1;
    const points = [s];
    for (let row = plan.a.row + sign; row !== plan.b.row; row += sign) {
      const boxes = rows.get(row);
      if (!boxes) continue;
      const box = boxes[0];
      const x = channel(row, s.x + (e.x - s.x) * (box.y + box.height / 2 - s.y) / (e.y - s.y));
      points.push({ x, y: sign > 0 ? box.y - 12 : box.y + box.height + 12 }, { x, y: sign > 0 ? box.y + box.height + 12 : box.y - 12 });
    }
    points.push(e);
    let d = `M ${round(s.x)} ${round(s.y)}`;
    for (let i = 0; i + 1 < points.length; i += 2) {
      const p = points[i], q = points[i + 1];
      const dy = Math.max(34, Math.abs(q.y - p.y) * 0.5) * sign;
      // Upward edges between neighbouring rows bow to the side, so they do
      // not overlap a downward edge between the same two cards.
      const bow = plan.dir === "up" && points.length === 2 ? 48 : 0;
      const curve = [p, { x: p.x + bow, y: p.y + dy }, { x: q.x + bow, y: q.y - dy }, q];
      plan.curves.push(curve);
      d += ` ${curveTo(curve)}`;
      if (i + 2 < points.length) d += ` L ${round(points[i + 2].x)} ${round(points[i + 2].y)}`;
    }
    plan.d = d;
  }
  return plans;
}
function freeRoute(plan) {
  const a = plan.a, b = plan.b;
  const ax = a.x + a.width / 2, ay = a.y + a.height / 2, bx = b.x + b.width / 2, by = b.y + b.height / 2;
  const lane = plan.edge.origin === "manual" ? 14 : 0;
  let s, e, c1, c2;
  if (Math.abs(by - ay) * 1.6 >= Math.abs(bx - ax)) {
    const down = by > ay ? 1 : -1;
    s = { x: ax + lane, y: down > 0 ? a.y + a.height : a.y };
    e = { x: bx + lane, y: down > 0 ? b.y - 2 : b.y + b.height + 2 };
    const d = Math.max(40, Math.abs(e.y - s.y) / 2) * down;
    c1 = { x: s.x, y: s.y + d }; c2 = { x: e.x, y: e.y - d };
  } else {
    const right = bx > ax ? 1 : -1;
    s = { x: right > 0 ? a.x + a.width : a.x, y: ay + lane };
    e = { x: right > 0 ? b.x - 2 : b.x + b.width + 2, y: by + lane };
    const d = Math.max(40, Math.abs(e.x - s.x) / 2) * right;
    c1 = { x: s.x + d, y: s.y }; c2 = { x: e.x - d, y: e.y };
  }
  plan.start = s; plan.end = e;
  plan.curves = [[s, c1, c2, e]];
  plan.d = `M ${round(s.x)} ${round(s.y)} ${curveTo(plan.curves[0])}`;
  return plan;
}
// A bus strand beside a chain of curves: each curve's ends and control
// points move along the normal at its ends (a close, cheap approximation).
function offsetCurves(curves, offset) {
  const normal = (a, b) => { const length = Math.hypot(b.x - a.x, b.y - a.y) || 1; return { x: -(b.y - a.y) / length, y: (b.x - a.x) / length }; };
  const move = (p, n) => ({ x: p.x + n.x * offset, y: p.y + n.y * offset });
  return curves.map(([p, c1, c2, q], index) => {
    const n0 = normal(p, Math.hypot(c1.x - p.x, c1.y - p.y) > 0.5 ? c1 : q), n3 = normal(Math.hypot(q.x - c2.x, q.y - c2.y) > 0.5 ? c2 : p, q);
    const curve = [move(p, n0), move(c1, n0), move(c2, n3), move(q, n3)];
    return `${index ? "L" : "M"} ${round(curve[0].x)} ${round(curve[0].y)} ${curveTo(curve)}`;
  }).join(" ");
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
    const width = plan.text.length * 7.2 + 8, height = 17;
    const curves = plan.curves.length > 1 ? [plan.curves[0], plan.curves[plan.curves.length - 1]] : plan.curves;
    plan.crowded = true;
    plan.label = bezier(curves[0], 0.5);
    for (const t of [0.5, 0.35, 0.65, 0.22, 0.78]) {
      for (const curve of curves) {
        const point = bezier(curve, t);
        const box = { x: point.x - width / 2, y: point.y - 9, width, height };
        if (overlaps(box)) continue;
        taken.push(box);
        plan.label = point;
        plan.crowded = false;
        break;
      }
      if (!plan.crowded) break;
    }
  }
}
// A revision cloud: the scalloped outline drafters draw around a change.
// Here it marks entries that take part in a rule violation.
function cloudPath(x, y, width, height) {
  const parts = [`M ${round(x)} ${round(y)}`];
  const side = (x0, y0, x1, y1) => {
    const length = Math.hypot(x1 - x0, y1 - y0), count = Math.max(1, Math.round(length / 17));
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

// ---------------------------------------------------------------- drawing
function nodeLines(node) {
  if (node.entry_kind === "file" && node.file_path) {
    const [directory, base] = splitPath(node.file_path);
    return [base, directory ? `${directory}/` : "", usageLabel(node) || null];
  }
  if (node.entry_kind === "group") {
    const flagged = node.members.filter((member) => member.violation_rule_ids.length).length;
    const idle = usageCount(node, "no_observed_users");
    return [node.title, node.direct ? `files in ${node.directory || "."}/` : `${node.directory}/`,
      `${plural(node.file_count, "file")}${flagged ? `, ${flagged} in violations` : ""}${idle ? `, ${idle} no users` : ""}`];
  }
  if (node.package) {
    const files = new Set((node.package.imports || []).map((item) => item.file)).size;
    return [node.title, `${ecosystemTitle(node.package.ecosystem)} package`, `imported by ${plural(files, "file")}`];
  }
  const facts = node.file_count || !node.package_count ? [plural(node.file_count, "file")] : [];
  if (hasValue(node.observed_file_count) && node.file_count) facts.push(`${node.observed_file_count} observed`);
  if (node.package_count) facts.push(plural(node.package_count, "package"));
  if (node.node_kind === "external") facts.push("external");
  return [node.title, node.architecture_id || ENTRY_KINDS[node.entry_kind] || node.entry_kind, facts.join(", ")];
}
// Card text is cut by its drawn width, not by a character count: a wide name
// such as `@fontsource/ibm-plex-sans` fits 26 characters but runs under the
// glyph in the card's corner. Measuring needs the card in the document, so
// drawNode leaves a character-count cut and a list of fits for afterwards.
function fitText(element, full, width, fromStart) {
  const cut = (length) => (fromStart ? compactStart(full, length) : compact(full, length));
  element.textContent = full;
  if (!element.getComputedTextLength()) { element.textContent = cut(fromStart ? 30 : 26); return; }
  if (element.getComputedTextLength() <= width) return;
  let low = 2, high = full.length - 1;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    element.textContent = cut(middle);
    if (element.getComputedTextLength() <= width) low = middle; else high = middle - 1;
  }
  element.textContent = cut(low);
}
// ---------------------------------------------------------------- wires
// How the canvas draws wires: the routing mode (curves, a PCB board with
// 0°/45°/90° traces, or a hexagonal board with 0°/60°/120° traces) and what
// gives a wire its colour. Remembered in the browser like the other views.
const MODES = ["curves", "pcb", "hex"], COLOURINGS = ["source", "kind", "target", "none"];
const display = (() => {
  const saved = stored("archgraph.view.v1", {}) || {};
  return { mode: MODES.includes(saved.mode) ? saved.mode : "curves", colour: COLOURINGS.includes(saved.colour) ? saved.colour : "source" };
})();
const layoutKind = () => (display.mode === "curves" ? "layout" : `layout-${display.mode}`);
// Six colour-blind-safe hues (Okabe–Ito, without its vermillion, which is
// kept for violations, and its black, which is the ink). Past six, the hues
// repeat with a dash pattern: 30 distinct looks.
const PALETTE = 6, DASHES = 5;
const KIND_ORDER = ["IMPORTS", "CALLS", "USES_CLASS", "FETCHES", "EXTENDS", "IMPLEMENTS"];
function netClass(index) {
  const cycle = Math.floor(index / PALETTE) % DASHES;
  return `n${index % PALETTE}${cycle ? ` c${cycle}` : ""}`;
}
// Source and target colourings give each entry with such edges a net, in
// the arrangement's reading order (so neighbours differ). The kind colouring
// takes the hues in a fixed order of kinds, so a level with few kinds gets
// the most distinct hues and the common kinds keep theirs across levels.
function assignColours(order) {
  const colours = { mode: display.colour, nets: new Map(), kinds: new Map() };
  if (display.colour === "source" || display.colour === "target") {
    const end = display.colour === "source" ? "from" : "to";
    const used = new Set(scene.layoutEdges.map((edge) => edge[end]));
    for (const id of order) if (used.has(id)) colours.nets.set(id, colours.nets.size);
  } else if (display.colour === "kind") {
    const rank = (kind) => (KIND_ORDER.includes(kind) ? KIND_ORDER.indexOf(kind) : KIND_ORDER.length);
    const kinds = [...new Set(scene.layoutEdges.flatMap((edge) => edge.parts.map((part) => part.kind)))].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
    kinds.forEach((kind, index) => colours.kinds.set(kind, index));
  }
  return colours;
}
// The colour of one edge: a net index, or strands (one per relation kind)
// for an edge of several kinds in the kind colouring.
function edgeLook(edge) {
  const colours = scene && scene.colours;
  if (!colours || colours.mode === "none") return { net: null };
  if (colours.mode === "kind") {
    const kinds = kindCounts(edge.parts).map(([kind]) => kind);
    if (kinds.length === 1) return { net: colours.kinds.has(kinds[0]) ? colours.kinds.get(kinds[0]) : null };
    return { net: null, strands: kinds.map((kind) => ({ kind, net: colours.kinds.get(kind) ?? null })) };
  }
  const net = colours.nets.get(colours.mode === "source" ? edge.from : edge.to);
  return { net: net === undefined ? null : net };
}
const strandSpacing = (count) => (count <= 3 ? 3 : 8 / (count - 1));
function busHalfWidth(edge) {
  const look = edgeLook(edge);
  return look.strands ? (look.strands.length - 1) / 2 * strandSpacing(look.strands.length) + 1 : 0;
}
function labelText(edge) {
  return `${edgeSummary(edge)}${edge.origin === "manual" ? " manual" : ""}${edge.suggested_cut_rule_ids.length ? " ✂" : edge.violation_rule_ids.length ? " ⚠" : ""}`;
}
// A short sample of an edge's wire, for the legend and the details panel.
function wireSample(look, { violating = false, manual = false, cut = false } = {}) {
  const sample = svg("svg", { viewBox: "0 0 30 12", width: 30, height: 12, "aria-hidden": "true",
    class: `swatch edge w2${manual ? " manual" : ""}${cut ? " cut" : ""}${violating ? " violating" : ""}${look.net !== null && look.net !== undefined ? ` ${netClass(look.net)}` : ""}${look.strands ? " bus" : ""}` });
  if (violating) sample.append(svg("path", { d: "M 2 6 H 28", class: "edge-casing" }), svg("path", { d: "M 2 6 H 28", class: "edge-gap" }));
  if (look.strands) {
    const spacing = strandSpacing(look.strands.length);
    look.strands.forEach((strand, i) => {
      const y = round(6 + (i - (look.strands.length - 1) / 2) * spacing);
      sample.append(svg("path", { d: `M 2 ${y} H 28`, class: `edge-strand ${strand.net === null ? "" : netClass(strand.net)}` }));
    });
  } else sample.append(svg("path", { d: "M 2 6 H 28", class: "edge-line" }));
  return sample;
}

// ---------------------------------------------------------------- drawing
function drawScene() {
  const graph = $("graph");
  graph.replaceChildren();
  const arrangement = arrange(scene.entries, scene.layoutEdges, scene.layers);
  scene.colours = assignColours(arrangement.order);
  const board = display.mode !== "curves" && scene.entries.length > 0;
  // Entries the user moved keep their place: a point on curves, a grid cell
  // on a board (each mode remembers its own).
  const saved = stored(storageKey(layoutKind(), scene.focusId), {}) || {};
  const moved = new Set();
  let positions, notes, left, plans;
  Object.assign(scene, { board: null, frameTraces: null, dropCell: null, ghost: null });
  if (board) {
    const fixed = new Map();
    for (const [id, place] of Object.entries(saved)) if (Array.isArray(place) && place.length === 2 && place.every(Number.isInteger)) fixed.set(id, place);
    const started = performance.now();
    const result = Board.route({ mode: display.mode, rows: arrangement.rows, centre: arrangement.centre, unit: CARD.width + GAP_X, card: CARD, fixed,
      edges: scene.edges.map((edge) => ({ from: edge.from, to: edge.to, halfWidth: busHalfWidth(edge) })),
      labels: scene.edges.length <= LABEL_LIMIT ? scene.edges.map((edge) => ({ width: labelText(edge).length * 6.6 + 10, height: 16,
        priority: (edge.violation_rule_ids.length ? 1e9 : 0) + edge.count })) : null });
    scene.routeTime = performance.now() - started;
    ({ positions, notes } = result);
    left = PAD + 24;
    scene.board = result;
    plans = result.traces.map((trace, order) => ({ edge: scene.edges[order], order, dir: trace.dir, points: trace.points, core: trace.core,
      d: Board.pathData(trace.points), label: trace.label, crowded: trace.label.crowded }));
    const inside = new Set(scene.entries.filter((node) => !node.outside_focus).map((node) => node.id));
    scene.frameTraces = plans.filter((plan) => inside.has(plan.edge.from) && inside.has(plan.edge.to)).flatMap((plan) => plan.points);
  } else {
    ({ positions, notes, left } = layout(arrangement));
    for (const [id, box] of positions) {
      const place = saved[id];
      if (Array.isArray(place) && place.length === 2 && place.every(Number.isFinite)) { box.x = place[0]; box.y = place[1]; moved.add(id); }
    }
  }
  Object.assign(scene, { positions, moved, nodeEls: new Map(), edgeEls: [] });
  graph.setAttribute("class", `mode-${display.mode} colour-${display.colour}${scene.edges.length > LABEL_LIMIT ? " quiet" : ""}${filters.violationsOnly ? " violations-only" : ""}${filters.idleOnly ? " idle-only" : ""}`);
  graph.setAttribute("aria-label", `${currentProjection.focus.title}: ${scene.entries.length} entries, ${scene.edges.length} directed dependencies`);
  if (!scene.entries.length) graph.append(svg("text", { x: 40, y: 70, class: "graph-note" }, "No mapped files or dependencies at this focus."));
  const frame = frameBox();
  if (frame) {
    scene.frameRect = svg("rect", { x: round(frame.x), y: round(frame.y), width: round(frame.width), height: round(frame.height), class: "frame" });
    scene.frameLabel = svg("text", { x: round(frame.x + 16), y: round(frame.y + 30), class: "frame-label" }, compact(currentProjection.focus.title, 60));
    graph.append(scene.frameRect, scene.frameLabel);
  }
  for (const note of notes) graph.append(svg("text", { x: left - 24, y: note.y, class: "graph-note" }, note.band === "callers" ? "Outside this focus, depending on it" : "Outside this focus"));
  if (!board) {
    plans = route(scene.edges, positions, moved);
    for (const plan of plans) plan.text = labelText(plan.edge);
    if (scene.edges.length <= LABEL_LIMIT) placeLabels(plans, positions);
    else for (const plan of plans) plan.label = bezier(plan.curves[0], 0.5);
  }
  for (const plan of plans) plan.text = labelText(plan.edge);
  // Violating edges are drawn last, on top of the others.
  plans.sort((a, b) => (a.edge.violation_rule_ids.length > 0) - (b.edge.violation_rule_ids.length > 0) || a.order - b.order);
  for (const plan of plans) graph.append(drawEdge(plan));
  scene.entries.forEach((node, index) => {
    const element = drawNode(node, positions.get(node.id), index === 0);
    graph.append(element);
    for (const fit of element.fits) fitText(...fit);
  });
  drawLegend();
}
// Marker ids: arrowheads (and vias on a board) take the wire's colour.
function markers(look, violating) {
  const coloured = look.net !== null && look.net !== undefined;
  return { end: violating ? "arrow-error" : coloured ? `arrow-n${look.net % PALETTE}` : "arrow", start: coloured ? `via-n${look.net % PALETTE}` : "via" };
}
// A bus: one strand per relation kind, side by side along the edge's course.
function strandPaths(plan, count) {
  const spacing = strandSpacing(count), paths = [];
  for (let i = 0; i < count; i++) {
    const offset = (i - (count - 1) / 2) * spacing;
    paths.push(plan.points ? Board.pathData(Board.offsetPolyline(plan.points, offset)) : offsetCurves(plan.curves, offset));
  }
  return paths;
}
function drawEdge(plan) {
  const edge = plan.edge;
  const violating = edge.violation_rule_ids.length > 0;
  const cut = edge.suggested_cut_rule_ids.length > 0;
  const weight = edge.count > 50 ? 4 : edge.count > 10 ? 3 : edge.count > 2 ? 2 : 1;
  const look = edgeLook(edge);
  const net = look.net !== null && look.net !== undefined ? ` ${netClass(look.net)}` : "";
  // Edges are not tab stops (a level can have hundreds); the keyboard
  // reaches them through the selected entry's dependency list.
  const group = svg("g", { class: `edge ${edge.origin} w${weight}${net}${look.strands ? " bus" : ""}${plan.dir === "up" ? " upward" : ""}${plan.crowded ? " crowded" : ""}${violating ? " violating" : ""}${cut ? " cut" : ""}`, "aria-hidden": "true" });
  const hit = svg("path", { d: plan.d, class: "edge-hit" });
  group.append(hit);
  // A violation keeps the wire's own colour inside a red casing.
  const casing = violating ? svg("path", { d: plan.d, class: "edge-casing" }) : null;
  if (casing) group.append(casing);
  // On a board, a casing of the sheet's colour cuts a gap where a later
  // trace crosses this one; inside a violation's red casing it separates the
  // red from the wire's own colour, so the outline reads as red on any hue.
  const gap = plan.points || violating ? svg("path", { d: plan.d, class: "edge-gap" }) : null;
  if (gap) group.append(gap);
  const ids = markers(look, violating);
  const line = svg("path", { d: plan.d, class: "edge-line", "marker-end": `url(#${ids.end})` });
  if (plan.points) line.setAttribute("marker-start", `url(#${ids.start})`);
  group.append(line);
  const strands = [];
  if (look.strands) {
    strandPaths(plan, look.strands.length).forEach((d, i) => {
      const strand = look.strands[i];
      strands.push(svg("path", { d, class: `edge-strand${strand.net === null ? "" : ` ${netClass(strand.net)}`}` }));
    });
    group.append(...strands);
    if (casing) casing.style.strokeWidth = `${round(2 * busHalfWidth(edge) + 8)}px`;
    if (gap) gap.style.strokeWidth = `${round(2 * busHalfWidth(edge) + (violating ? 4 : 5))}px`;
  }
  const label = svg("text", { x: round(plan.label.x), y: round(plan.label.y + 4.5), class: "edge-label" }, plan.text);
  group.append(label);
  group.append(svg("title", {}, `${displayEndpoint(edge.from)} → ${displayEndpoint(edge.to)}\n${kindCounts(edge.parts).map(([kind, count]) => `${kind} × ${count}`).join(", ")}. Click for concrete evidence.`));
  group.addEventListener("click", () => showEdge(edge, group));
  group.addEventListener("mouseenter", () => hoverCanvas({ nodes: new Set([edge.from, edge.to]), edges: new Set([edge]) }, { edge }));
  group.addEventListener("mouseleave", () => hoverCanvas(null));
  const marker = `url(#${ids.end})`;
  // Without colours a highlighted wire turns to ink; with them it keeps its colour.
  const litMarker = !violating && (display.colour === "none" || look.strands) ? "url(#arrow-lit)" : marker;
  scene.edgeEls.push({ edge, core: plan.core || null, element: group, line, hit, casing, gap, strands, label, marker, litMarker, violating, count: look.strands ? look.strands.length : 0 });
  return group;
}
// Puts an edge on a new course (a dragged entry's rubber band).
function reshapeEdge(item, plan) {
  for (const path of [item.line, item.hit, item.casing, item.gap]) if (path) path.setAttribute("d", plan.d);
  if (item.strands.length) strandPaths(plan, item.count).forEach((d, i) => item.strands[i].setAttribute("d", d));
  const point = bezier(plan.curves[0], 0.5);
  item.label.setAttribute("x", round(point.x));
  item.label.setAttribute("y", round(point.y + 4.5));
}
function usageMark(node, box) {
  const entries = usageCount(node, "entry_point"), idle = usageCount(node, "no_observed_users");
  const unused = node.no_outside_users || node.usage === "no_observed_users";
  if (!entries && !idle && !unused) return null;
  const element = svg("g", { class: "usage-mark", transform: `translate(${box.width - 34}, 10)`, "aria-hidden": "true" });
  if (entries && !unused) {
    element.classList.add("entry");
    element.append(svg("circle", { cx: 12, cy: 12, r: 10, class: "entry-ring" }), svg("path", { d: "M9.5 7.5 L17 12 L9.5 16.5 Z", class: "entry-play" }));
    return { element, width: 28 };
  }
  element.classList.add("idle");
  element.append(svg("circle", { cx: 12, cy: 12, r: 9, class: "idle-ring" }));
  // A node with some such files, not unused as a whole: how many.
  const count = node.entry_kind === "file" || node.no_outside_users ? "" : String(idle);
  if (count) element.append(svg("text", { x: -3, y: 17, class: "idle-count" }, count));
  return { element, width: 28 + (count ? count.length * 9 + 4 : 0) };
}
function drawNode(node, box, first) {
  const violating = node.violation_rule_ids.length > 0;
  const status = usageLabel(node);
  const entryPoint = usageCount(node, "entry_point") > 0;
  const group = svg("g", { class: `node ${node.entry_kind}${node.outside_focus ? " outside" : ""}${node.node_kind === "external" ? " external" : ""}${violating ? " violating" : ""}${isIdle(node) ? " idle" : ""}${entryPoint ? " entry-point" : ""}${node.usage === "not_indexed" ? " unknown" : ""}`,
    transform: `translate(${round(box.x)}, ${round(box.y)})`, tabindex: first ? 0 : -1, role: "button", "aria-label": status ? `${node.title}, ${status}` : node.title, "data-id": node.id });
  // On the hexagonal board a card is a plate with pointed ends; its text
  // stays horizontal inside the rectangle between them.
  const ear = box.hex || 0;
  const shape = (x, y, className) => (ear
    ? svg("path", { d: Board.hexOutline(box.width, box.height), transform: `translate(${x}, ${y})`, class: className })
    : svg("rect", { x, y, width: box.width, height: box.height, class: className }));
  if (node.entry_kind === "group") group.append(shape(8, 8, "sheet-back"), shape(4, 4, "sheet-back"));
  if (violating) group.append(svg("path", { d: cloudPath(-8 - ear, -8, box.width + 16 + 2 * ear + (node.entry_kind === "group" ? 8 : 0), box.height + 16 + (node.entry_kind === "group" ? 8 : 0)), class: "cloud" }));
  group.append(shape(0, 0, "box"));
  // The entry's net colour, when wires are coloured by source or target.
  const net = scene && scene.colours && scene.colours.nets.get(node.id);
  if (net !== undefined && net !== null) group.append(svg("rect", { x: 0, y: 0, width: 5, height: box.height, class: `net-tab ${netClass(net)}` }));
  if (node.entry_kind === "architecture") group.append(svg("rect", { x: 4, y: 4, width: box.width - 8, height: box.height - 8, class: "box-inner" }));
  // A package is a bought-in part: a crate glyph in the corner.
  if (node.entry_kind === "package") group.append(svg("path", { d: `M ${box.width - 34} 16 l 10 -5 l 10 5 v 12 l -10 5 l -10 -5 z M ${box.width - 34} 16 l 10 5 l 10 -5 M ${box.width - 24} 21 v 12`, class: "package-glyph" }));
  const [title, subtitle, facts] = nodeLines(node);
  const titleText = svg("text", { x: 14, y: facts === null ? 37 : 30, class: "node-title" }, compact(title, 26));
  const subtitleText = svg("text", { x: 14, y: facts === null ? 60 : 51, class: "node-subtitle" }, compactStart(subtitle, 30));
  group.append(titleText, subtitleText);
  // Entry points and entries with no observed users carry a corner mark:
  // a start glyph, or a dashed ring (with the count of such files).
  const mark = node.entry_kind === "group" || node.entry_kind === "package" ? null : usageMark(node, box);
  if (mark) group.append(mark.element);
  // The corner glyph or group toggle starts 34px from the right edge.
  const corner = node.entry_kind === "package" || node.entry_kind === "group" ? 54 : mark ? 26 + mark.width : 26;
  group.fits = [[titleText, title, box.width - corner, false], [subtitleText, subtitle, box.width - 26, node.entry_kind === "file" || node.entry_kind === "group"]];
  if (facts) {
    const usageClass = node.entry_kind === "file" && node.usage ? ` usage-${node.usage}` : "";
    const factsText = svg("text", { x: 14, y: 70, class: `node-facts${usageClass}` }, compact(facts, 30));
    group.append(factsText);
    group.fits.push([factsText, facts, box.width - 26, false]);
  }
  // Share of files with any observed dependency: low coverage makes a clean
  // check weak evidence, so it is shown on every architecture card.
  if (hasValue(node.observed_file_count) && node.file_count > 0) {
    const track = box.width - 28;
    group.append(svg("rect", { x: 14, y: 76, width: track, height: 3, class: "coverage-track" }));
    group.append(svg("rect", { x: 14, y: 76, width: round(track * Math.min(1, node.observed_file_count / node.file_count)), height: 3, class: "coverage" }));
  }
  if (node.entry_kind === "group") {
    const toggle = svg("g", { class: "group-toggle", transform: `translate(${box.width - 34}, 10)` });
    toggle.append(svg("rect", { width: 24, height: 24 }), svg("path", { d: "M7 12h10M12 7v10" }), svg("title", {}, "Expand this group"));
    toggle.addEventListener("click", (event) => { event.stopPropagation(); setGroupOpen(node, true); });
    group.append(toggle);
  }
  group.append(svg("title", {}, [node.title, node.architecture_id || node.file_path, node.description, status ? `Usage: ${status}` : null,
    hasValue(node.observed_file_count) && node.file_count ? `${node.observed_file_count} of ${node.file_count} files have an observed dependency` : null,
    node.package ? `${ecosystemTitle(node.package.ecosystem)} package ${node.id}` : null,
    node.entry_kind === "group" ? "Double-click to expand" : node.entry_kind === "architecture" ? "Double-click to open" : null].filter(Boolean).join("\n")));
  group.addEventListener("click", () => showNode(node, group));
  group.addEventListener("dblclick", () => {
    if (node.entry_kind === "architecture") loadFocus(node.architecture_id);
    else if (node.entry_kind === "group") setGroupOpen(node, true);
  });
  group.addEventListener("mouseenter", () => { if (!gesture) hoverCanvas(neighbourhood(node.id), { node: node.id }); });
  group.addEventListener("mouseleave", () => { if (!gesture) hoverCanvas(null); });
  group.addEventListener("focus", () => hoverCanvas(neighbourhood(node.id), { node: node.id }));
  group.addEventListener("blur", () => hoverCanvas(null));
  group.addEventListener("keydown", (event) => nodeKey(event, node, group));
  scene.nodeEls.set(node.id, group);
  return group;
}
// Moves an entry while it is dragged; its edges follow as curves. On a
// board they are rubber bands until the drop, and a ghost shows the grid
// cell the entry will snap to.
function moveEntry(id, x, y) {
  const box = scene.positions.get(id);
  box.x = x;
  box.y = y;
  if (!scene.board) scene.moved.add(id);
  scene.nodeEls.get(id).setAttribute("transform", `translate(${round(x)}, ${round(y)})`);
  for (const item of scene.edgeEls) {
    if (item.edge.from !== id && item.edge.to !== id) continue;
    reshapeEdge(item, freeRoute({ edge: item.edge, a: scene.positions.get(item.edge.from), b: scene.positions.get(item.edge.to) }));
    item.line.removeAttribute("marker-start");
    item.element.classList.remove("crowded");
    if (scene.board) item.element.classList.add("rubber");
  }
  if (scene.board) {
    const cell = Board.snap(scene.board.grid, x + box.width / 2, y + box.height / 2);
    const slot = Board.slotBox(scene.board.grid, cell.row, cell.col);
    if (!scene.ghost) {
      scene.ghost = svg("path", { class: "drop-slot" });
      $("graph").insertBefore(scene.ghost, $("graph").firstChild);
    }
    scene.ghost.setAttribute("transform", `translate(${round(slot.x)}, ${round(slot.y)})`);
    scene.ghost.setAttribute("d", slot.hex ? Board.hexOutline(slot.width, slot.height) : `M 0 0 H ${round(slot.width)} V ${slot.height} H 0 Z`);
    scene.dropCell = cell;
  }
  const frame = frameBox();
  if (frame && scene.frameRect) {
    for (const key of ["x", "y", "width", "height"]) scene.frameRect.setAttribute(key, round(frame[key]));
    scene.frameLabel.setAttribute("x", round(frame.x + 16));
    scene.frameLabel.setAttribute("y", round(frame.y + 30));
  }
  const mark = $("minimap").querySelector(`[data-id="${CSS.escape(id)}"]`);
  if (mark) { mark.setAttribute("x", round(x)); mark.setAttribute("y", round(y)); }
}
function saveLayout() {
  const saved = {};
  for (const id of [...scene.moved].sort()) {
    const box = scene.positions.get(id);
    if (box) saved[id] = [round(box.x), round(box.y)];
  }
  store(storageKey("layout", scene.focusId), saved);
}
// A board drop: the entry takes the cell under it, an entry already there
// takes the cell it left, and the whole board is routed again.
function dropOnGrid(id) {
  const cell = scene.dropCell, box = scene.positions.get(id);
  if (!cell || !box) { redraw(); return; }
  const key = storageKey(layoutKind(), scene.focusId);
  const saved = { ...(stored(key, {}) || {}) };
  for (const [other, place] of scene.positions) {
    if (other !== id && place.row === cell.row && place.col === cell.col) saved[other] = [box.row, box.col];
  }
  saved[id] = [cell.row, cell.col];
  store(key, Object.fromEntries(Object.keys(saved).sort().map((other) => [other, saved[other]])));
  const before = new Map([...scene.positions].map(([other, place]) => [other, { ...place }]));
  redraw();
  settle(before, null);
}
function redraw() {
  if (!currentProjection || view !== "diagram") return;
  scene = buildScene(currentProjection);
  drawScene();
  drawMinimap();
  applyCamera();
  restoreSelection();
  updateFilterUI();
}
// Entries glide from where they were to their new place after a group opens
// or closes or the layout is reset; new entries start from the group.
function settle(before, anchor) {
  if (reducedMotion.matches || !scene || !scene.nodeEls) return;
  const moves = [];
  for (const [id, element] of scene.nodeEls) {
    const to = scene.positions.get(id), from = before.get(id) || anchor;
    if (from && (from.x !== to.x || from.y !== to.y)) moves.push({ element, from, to });
  }
  if (!moves.length) return;
  $("graph").classList.add("settling");
  const began = performance.now();
  const step = (now) => {
    const t = Math.min(1, (now - began) / 340), eased = 1 - Math.pow(1 - t, 3);
    for (const move of moves) {
      move.element.setAttribute("transform", `translate(${round(move.from.x + (move.to.x - move.from.x) * eased)}, ${round(move.from.y + (move.to.y - move.from.y) * eased)})`);
    }
    if (t < 1) requestAnimationFrame(step); else $("graph").classList.remove("settling");
  };
  requestAnimationFrame(step);
}

// ---------------------------------------------------------------- camera
function stageSize() {
  const rect = $("stage").getBoundingClientRect();
  return { width: rect.width || 800, height: rect.height || 600, left: rect.left, top: rect.top };
}
function applyCamera() {
  const transform = `translate(${round(camera.x)} ${round(camera.y)}) scale(${camera.k.toFixed(4)})`;
  $("graph").setAttribute("transform", transform);
  $("grid").setAttribute("patternTransform", transform);
  $("stage").classList.toggle("coarse", camera.k < 0.45);
  $("zoom-level").textContent = `${Math.round(camera.k * 100)}%`;
  updateMinimapView();
}
function setCamera(target) {
  ++cameraAnimation;
  camera = { ...target };
  applyCamera();
}
function zoomAt(factor, px, py) {
  ++cameraAnimation;
  const k = clamp(camera.k * factor, MIN_ZOOM, MAX_ZOOM);
  const wx = (px - camera.x) / camera.k, wy = (py - camera.y) / camera.k;
  camera = { k, x: px - wx * k, y: py - wy * k };
  applyCamera();
}
// The camera that shows `box` whole, clear of the floating tools.
function cameraFor(box, { maxK = 1, minK = MIN_ZOOM, pad = 48, top = false } = {}) {
  const stage = stageSize(), padTop = pad + 84; // clear of the two rows of tools
  // An open legend keeps its corner: the view is fitted beside it.
  const legend = $("legend");
  const reserve = pad > 0 && !legend.hidden && legend.open ? legend.offsetWidth + 12 : 0;
  const size = { ...stage, width: Math.max(stage.width / 2, stage.width - reserve) };
  const k = clamp(Math.min((size.width - 2 * pad) / box.width, (size.height - padTop - pad) / box.height), minK, maxK);
  let x = (size.width - box.width * k) / 2 - box.x * k;
  // Too wide to show whole: start at its left edge, where the labels are.
  if (top && box.width * k > size.width - 2 * pad) x = pad - box.x * k;
  let y = padTop + (size.height - padTop - pad - box.height * k) / 2 - box.y * k;
  if (top && box.height * k > size.height - padTop - pad) y = padTop - box.y * k;
  return { x, y, k };
}
// Opening a level: everything if it fits at a readable size (75% or more),
// else its full width from the top, at 60% or more. "Fit" always shows
// everything.
function initialCamera() {
  const bounds = contentBounds();
  const whole = cameraFor(bounds, { maxK: 1 });
  if (whole.k >= 0.75) return whole;
  const size = stageSize();
  return cameraFor({ ...bounds, height: Math.min(bounds.height, (size.height - 120) / clamp((size.width - 96) / bounds.width, 0.6, 1)) }, { maxK: 1, minK: 0.6, top: true });
}
function fitView() { if (scene && scene.positions) animateTo(cameraFor(contentBounds(), { maxK: 1.25 })); }
function zoomToSelection() {
  if (!scene || !scene.positions) return;
  const ids = selection ? [...selection.nodes].filter((id) => scene.positions.has(id)) : [];
  if (!ids.length) { fitView(); return; }
  const boxes = ids.map((id) => scene.positions.get(id));
  const x = Math.min(...boxes.map((b) => b.x)), y = Math.min(...boxes.map((b) => b.y));
  const box = { x, y, width: Math.max(...boxes.map((b) => b.x + b.width)) - x, height: Math.max(...boxes.map((b) => b.y + b.height)) - y };
  animateTo(cameraFor(box, { maxK: 1.5, pad: 72 }));
}
function centreOn(id, minK = 0.9) {
  const box = scene.positions.get(id);
  if (!box) return;
  const size = stageSize(), k = Math.max(camera.k, minK);
  animateTo({ k, x: size.width / 2 - (box.x + box.width / 2) * k, y: size.height / 2 - (box.y + box.height / 2) * k });
}
function animateTo(target, duration = 320) {
  const token = ++cameraAnimation;
  if (reducedMotion.matches || !duration) { camera = { ...target }; applyCamera(); return Promise.resolve(); }
  const start = { ...camera }, began = performance.now();
  return new Promise((resolve) => {
    const step = (now) => {
      if (token !== cameraAnimation) { resolve(); return; }
      const t = Math.min(1, (now - began) / duration);
      const eased = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
      camera = { k: start.k * Math.pow(target.k / start.k, eased), x: start.x + (target.x - start.x) * eased, y: start.y + (target.y - start.y) * eased };
      applyCamera();
      if (t < 1) requestAnimationFrame(step); else resolve();
    };
    requestAnimationFrame(step);
  });
}

// ---------------------------------------------------------------- minimap
function drawMinimap() {
  const map = $("minimap");
  map.replaceChildren();
  if (!scene || !scene.positions || !scene.positions.size) return;
  const bounds = contentBounds(), pad = 60;
  map.setAttribute("viewBox", `${round(bounds.x - pad)} ${round(bounds.y - pad)} ${round(bounds.width + 2 * pad)} ${round(bounds.height + 2 * pad)}`);
  for (const node of scene.entries) {
    const box = scene.positions.get(node.id);
    map.append(svg("rect", { x: round(box.x), y: round(box.y), width: box.width, height: box.height, "data-id": node.id,
      class: `mm-node${node.violation_rule_ids.length ? " violating" : ""}${node.outside_focus ? " outside" : ""}` }));
  }
  map.append(svg("rect", { id: "minimap-view", class: "mm-view" }));
  updateMinimapView();
}
function updateMinimapView() {
  const frame = $("minimap-view");
  if (!frame) return;
  const size = stageSize();
  frame.setAttribute("x", round(-camera.x / camera.k));
  frame.setAttribute("y", round(-camera.y / camera.k));
  frame.setAttribute("width", round(size.width / camera.k));
  frame.setAttribute("height", round(size.height / camera.k));
}
let minimapDrag = false;
function minimapMove(event) {
  const map = $("minimap");
  const matrix = map.getScreenCTM();
  if (!matrix || !scene) return;
  const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse());
  const size = stageSize();
  setCamera({ k: camera.k, x: size.width / 2 - point.x * camera.k, y: size.height / 2 - point.y * camera.k });
}
$("minimap").addEventListener("pointerdown", (event) => { minimapDrag = true; $("minimap").setPointerCapture(event.pointerId); minimapMove(event); });
$("minimap").addEventListener("pointermove", (event) => { if (minimapDrag) minimapMove(event); });
$("minimap").addEventListener("pointerup", () => { minimapDrag = false; });

// ---------------------------------------------------------------- canvas input
// Drag empty space (or anything while Space is held) to pan, drag an entry to
// move it, wheel to zoom at the pointer, two-finger scroll to pan, pinch to zoom.
const pointers = new Map();
let gesture = null, suppressClick = false, spacePanned = false;
const stage = $("stage");
stage.addEventListener("pointerdown", (event) => {
  if (view !== "diagram" || (event.pointerType === "mouse" && event.button !== 0 && event.button !== 1)) return;
  pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
  if (pointers.size === 2) {
    const [a, b] = [...pointers.values()];
    gesture = { kind: "pinch", distance: Math.hypot(a.x - b.x, a.y - b.y), mid: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }, moved: true };
    return;
  }
  const node = event.target.closest && event.target.closest("#graph .node");
  if (node && !spaceHeld && event.button === 0 && !event.target.closest(".group-toggle")) {
    gesture = { kind: "node", id: node.dataset.id, x: event.clientX, y: event.clientY, origin: { ...scene.positions.get(node.dataset.id) }, moved: false };
  } else {
    gesture = { kind: "pan", x: event.clientX, y: event.clientY, camera: { ...camera }, moved: false };
  }
});
stage.addEventListener("pointermove", (event) => {
  if (!pointers.has(event.pointerId)) return;
  pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
  if (!gesture) return;
  if (gesture.kind === "pinch") {
    if (pointers.size < 2) return;
    const [a, b] = [...pointers.values()];
    const distance = Math.hypot(a.x - b.x, a.y - b.y), mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
    const size = stageSize();
    zoomAt(distance / (gesture.distance || distance), mid.x - size.left, mid.y - size.top);
    camera = { ...camera, x: camera.x + mid.x - gesture.mid.x, y: camera.y + mid.y - gesture.mid.y };
    applyCamera();
    Object.assign(gesture, { distance, mid });
    return;
  }
  const dx = event.clientX - gesture.x, dy = event.clientY - gesture.y;
  if (!gesture.moved) {
    if (Math.hypot(dx, dy) < 4) return;
    gesture.moved = true;
    ++cameraAnimation;
    stage.setPointerCapture(event.pointerId);
    $("canvas").classList.add(gesture.kind === "node" ? "dragging" : "panning");
    paint(null, "hover", "has-hover");
  }
  if (gesture.kind === "pan") {
    if (spaceHeld) spacePanned = true;
    camera = { ...camera, x: gesture.camera.x + dx, y: gesture.camera.y + dy };
    applyCamera();
  } else {
    moveEntry(gesture.id, gesture.origin.x + dx / camera.k, gesture.origin.y + dy / camera.k);
  }
});
function endGesture(event) {
  pointers.delete(event.pointerId);
  if (!gesture) return;
  if (gesture.moved) {
    suppressClick = true;
    setTimeout(() => { suppressClick = false; }, 0);
    if (gesture.kind === "node") { if (scene.board) dropOnGrid(gesture.id); else saveLayout(); }
  }
  $("canvas").classList.remove("dragging", "panning");
  gesture = null;
}
stage.addEventListener("pointerup", endGesture);
stage.addEventListener("pointercancel", endGesture);
stage.addEventListener("click", (event) => {
  if (suppressClick) { event.stopPropagation(); event.preventDefault(); return; }
  if (!event.target.closest("#graph .node, #graph .edge") && selected) showOverview();
}, true);
stage.addEventListener("wheel", (event) => {
  if (view !== "diagram") return;
  event.preventDefault();
  const size = stageSize();
  const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? size.height : 1;
  const dx = event.deltaX * unit, dy = event.deltaY * unit;
  // A mouse wheel notch zooms; a trackpad's small, fractional or sideways
  // deltas pan; a pinch arrives as a wheel event with Ctrl held.
  const notch = event.deltaMode !== 0 || (dx === 0 && Math.abs(dy) >= 50 && Number.isInteger(dy));
  if (event.ctrlKey || event.metaKey || notch) zoomAt(Math.exp(-dy * (event.ctrlKey ? 0.01 : 0.0015)), event.clientX - size.left, event.clientY - size.top);
  else { ++cameraAnimation; camera = { ...camera, x: camera.x - dx, y: camera.y - dy }; applyCamera(); }
}, { passive: false });
// Keyboard: the canvas is one tab stop; arrow keys move between entries,
// Enter shows details, Shift+Enter opens a node or expands a group.
function nodeKey(event, node, element) {
  if (event.key === "Enter") {
    event.preventDefault();
    if (event.shiftKey && node.entry_kind === "architecture") loadFocus(node.architecture_id);
    else if (event.shiftKey && node.entry_kind === "group") setGroupOpen(node, true);
    else showNode(node, element);
    return;
  }
  const directions = { ArrowRight: [1, 0], ArrowLeft: [-1, 0], ArrowDown: [0, 1], ArrowUp: [0, -1] };
  let target = null;
  if (event.key === "Home" || event.key === "End") {
    const ids = [...scene.nodeEls.keys()].sort((a, b) => {
      const p = scene.positions.get(a), q = scene.positions.get(b);
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
  const here = scene.positions.get(id);
  let best = null, bestScore = Infinity;
  for (const [other, box] of scene.positions) {
    const along = (box.x - here.x) * dx + (box.y - here.y) * dy;
    if (other === id || along <= 0 || !scene.nodeEls.has(other)) continue;
    const score = along + 2 * (Math.abs((box.x - here.x) * dy) + Math.abs((box.y - here.y) * dx));
    if (score < bestScore) { bestScore = score; best = other; }
  }
  return best;
}
function moveTo(id) {
  for (const element of scene.nodeEls.values()) element.setAttribute("tabindex", "-1");
  const element = scene.nodeEls.get(id);
  element.setAttribute("tabindex", "0");
  element.focus({ preventScroll: true });
  const box = scene.positions.get(id), size = stageSize();
  const x = box.x * camera.k + camera.x, y = box.y * camera.k + camera.y;
  if (x < 0 || y < 60 || x + box.width * camera.k > size.width || y + box.height * camera.k > size.height) centreOn(id, camera.k);
}
const typing = (target) => target.closest && target.closest("input, select, textarea, [contenteditable]");
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    closePopovers();
    if ($("search-results").childElementCount) { closeSearch(); return; }
    if (narrow.matches && drawer) { togglePanel(drawer, false); return; }
    if (selected && !typing(event.target)) showOverview();
    return;
  }
  if (typing(event.target)) return;
  if (event.key === "/" && !event.ctrlKey && !event.metaKey && !event.altKey) { event.preventDefault(); $("search").focus(); return; }
  if (view !== "diagram" || event.ctrlKey || event.metaKey || event.altKey) return;
  const control = event.target.closest && event.target.closest("button, a, summary, [role='treeitem']");
  if (event.key === " " && !control) {
    event.preventDefault();
    if (!spaceHeld) { spaceHeld = true; spacePanned = false; $("canvas").classList.add("space-pan"); }
    return;
  }
  if (control) return;
  const size = stageSize();
  if (event.key === "+" || event.key === "=") zoomAt(1.25, size.width / 2, size.height / 2);
  else if (event.key === "-" || event.key === "_") zoomAt(0.8, size.width / 2, size.height / 2);
  else if (event.shiftKey && event.code === "Digit1") fitView();
  else if (event.shiftKey && event.code === "Digit2") zoomToSelection();
  else return;
  event.preventDefault();
});
document.addEventListener("keyup", (event) => {
  if (event.key !== " " || !spaceHeld) return;
  spaceHeld = false;
  $("canvas").classList.remove("space-pan");
  // Space pressed and released on an entry without panning activates it.
  const node = event.target.closest && event.target.closest("#graph .node");
  if (node && !spacePanned && scene.byId.has(node.dataset.id)) showNode(scene.byId.get(node.dataset.id), node);
});

// ---------------------------------------------------------------- views
function renderView() {
  if (!currentProjection) return;
  view = viewChoice || "diagram";
  $("view-diagram").setAttribute("aria-pressed", String(view === "diagram"));
  $("view-table").setAttribute("aria-pressed", String(view === "table"));
  $("canvas").classList.toggle("table-mode", view === "table");
  $("table-wrap").hidden = view !== "table";
  if (view === "diagram") {
    $("table-wrap").replaceChildren();
    scene = buildScene(currentProjection);
    drawScene();
    drawMinimap();
  } else {
    scene = null;
    $("graph").replaceChildren();
    $("minimap").replaceChildren();
    $("legend").hidden = true;
    drawTable(currentProjection);
  }
  updateFilterUI();
}
function setView(next) {
  viewChoice = next;
  renderView();
  if (view === "diagram") setCamera(initialCamera());
}
// Every entry of the level with its dependency counts, file by file: a
// filterable index next to the canvas.
const table = { filter: "", sort: "path", timer: null };
function tableGroup(node) {
  if (node.outside_focus) return "Outside this focus";
  if (node.entry_kind === "architecture") return "Child nodes";
  if (node.entry_kind === "package") return "Packages";
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
  for (const [value, text] of [["path", "Group by directory"], ["links", "Most dependencies first"], ["rules", "Violations first"], ["idle", "No observed users first"]]) {
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
  for (const [text, className] of [["Entry", "entry-col"], ["Usage", "usage-col"], ["Depends on", "num"], ["Used by", "num"], ["Violations", "num"]]) {
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
  const rank = (node) => ({ "Child nodes": 0, "Packages": 1, "This node": 2, "Outside this focus": 4 })[tableGroup(node)] ?? 3;
  const name = (node) => node.package ? node.id : node.file_path || node.architecture_id || node.title;
  const render = () => {
    const query = table.filter.trim().toLowerCase();
    const grouped = table.sort === "path";
    const shown = projection.nodes.filter((node) => !query || [node.title, node.file_path, node.architecture_id, usageLabel(node)].some((value) => value && value.toLowerCase().includes(query)));
    const idleRank = (node) => node.no_outside_users ? 1e9 : usageCount(node, "no_observed_users");
    shown.sort((a, b) => (grouped ? rank(a) - rank(b) || tableGroup(a).localeCompare(tableGroup(b)) : 0)
      || (table.sort === "rules" ? b.violation_rule_ids.length - a.violation_rule_ids.length : 0)
      || (table.sort === "idle" ? idleRank(b) - idleRank(a) : 0)
      || (grouped ? 0 : links(b) - links(a)) || name(a).localeCompare(name(b)));
    body.replaceChildren();
    let group = null;
    for (const node of shown) {
      if (grouped && tableGroup(node) !== group) {
        group = tableGroup(node);
        const row = html("tr", null, "group-row");
        const cell = html("th", group);
        cell.colSpan = 5;
        cell.scope = "colgroup";
        row.append(cell);
        body.append(row);
      }
      body.append(tableRow(node, stats.get(node.id), grouped));
    }
    if (!shown.length) {
      const row = html("tr");
      const cell = html("td", query ? "No entries match this filter." : "No mapped files or dependencies at this focus.", "empty");
      cell.colSpan = 5;
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
  const row = html("tr", null, `entry-row${node.outside_focus ? " outside" : ""}${node.violation_rule_ids.length ? " violating" : ""}${isIdle(node) ? " idle" : ""}`);
  if ($("details").dataset.entry === node.id) row.classList.add("selected");
  const file = node.entry_kind === "file" && node.file_path;
  const [directory, base] = file ? splitPath(node.file_path) : ["", ""];
  const button = html("button", null, `entry-button ${node.entry_kind}`);
  button.type = "button";
  button.append(html("span", file ? base : node.title, "entry-title"));
  const sub = file ? (grouped ? "" : directory) : node.package ? node.id : node.architecture_id || ENTRY_KINDS[node.entry_kind];
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
  const usage = usageLabel(node);
  const usageCell = html("td", usage, `usage-cell${node.usage ? ` usage-${node.usage}` : isIdle(node) ? " usage-no_observed_users" : ""}`);
  row.append(cell, usageCell, html("td", stat.out || "", "num"), html("td", stat.in || "", "num"),
    html("td", node.violation_rule_ids.length || "", "num rules"));
  row.addEventListener("click", () => { showNode(node, row); $("details").dataset.entry = node.id; });
  row.addEventListener("dblclick", () => { if (node.entry_kind === "architecture") loadFocus(node.architecture_id); });
  return row;
}

// ---------------------------------------------------------------- search
function closeSearch() {
  ++searchRequest;
  $("search-results").replaceChildren();
}
function resultButton(title, sub, label, action) {
  const button = html("button", null, "search-result");
  button.type = "button";
  button.setAttribute("aria-label", label);
  button.append(html("span", title, "result-title"), html("span", sub, "result-id"));
  button.addEventListener("click", () => { closeSearch(); $("search").value = ""; action(); });
  return button;
}
// Entries of the current level (files included) first, then architecture
// nodes anywhere in the repository.
async function searchNodes() {
  const request = ++searchRequest;
  const query = $("search").value.trim();
  $("search-results").replaceChildren();
  if (!query) return;
  const lower = query.toLowerCase();
  const local = currentProjection ? currentProjection.nodes.filter((node) => [node.title, node.file_path, node.architecture_id]
    .some((value) => value && value.toLowerCase().includes(lower))).slice(0, 8) : [];
  try {
    const results = await api(`/api/search?q=${encodeURIComponent(query)}`);
    if (request !== searchRequest) return;
    const box = $("search-results");
    if (local.length) {
      box.append(html("p", "In this view", "result-heading"));
      for (const node of local) {
        const status = usageLabel(node);
        const where = node.file_path || node.architecture_id || entryKind(node);
        box.append(resultButton(entryTitle(node), status ? `${where} · ${status}` : where, `${entryTitle(node)} (in this view)${status ? `, ${status}` : ""}`, () => revealEntry(node.id)));
      }
    }
    box.append(html("p", "Architecture nodes", "result-heading"));
    if (!results.length) box.append(html("p", "No matching architecture nodes.", "muted"));
    for (const node of results) {
      const status = usageLabel({ ...node, entry_kind: "architecture" });
      box.append(resultButton(node.title, status ? `${node.id} · ${status}` : node.id, `${node.title} — ${node.id}${status ? `, ${status}` : ""}`, () => loadFocus(node.id)));
    }
    // Imported packages anywhere: opening one shows who imports it.
    const packages = allPackages.filter((item) => item.name.toLowerCase().includes(lower) || item.id.toLowerCase().includes(lower)).slice(0, 12);
    if (packages.length) {
      box.append(html("p", "Packages", "result-heading"));
      for (const item of packages) {
        const where = `${ecosystemTitle(item.ecosystem)} · ${plural(item.file_count, "importing file")}`;
        box.append(resultButton(item.name, where, `${item.name} — ${ecosystemTitle(item.ecosystem)} package`, () => openPackage(item)));
      }
    }
  } catch (error) { if (request === searchRequest) showError(error); }
}
// Shows a package selected at the level of the node owning it.
async function openPackage(item) {
  if (!item.node) return;
  if (narrow.matches) togglePanel("sidebar", false);
  if (currentProjection && currentProjection.focus.id === item.node && entries.has(item.id)) { revealEntry(item.id); return; }
  if (view !== "diagram") setView("diagram");
  await loadFocus(item.node, true, { select: item.id });
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

// ---------------------------------------------------------------- filters
function updateFilterUI() {
  $("filter-violations").checked = filters.violationsOnly;
  $("filter-idle").checked = filters.idleOnly;
  $("filter-outside").checked = filters.outside;
  $("filter-observed").checked = filters.observed;
  $("filter-manual").checked = filters.manual;
  const kinds = currentProjection ? kindCounts(currentProjection.edges) : [];
  const box = $("filter-kinds");
  box.replaceChildren();
  if (!kinds.length) box.append(html("p", "No dependencies at this level.", "muted"));
  for (const [kind, count] of kinds) {
    const label = html("label");
    const input = html("input");
    input.type = "checkbox";
    input.dataset.kind = kind;
    input.checked = !filters.hiddenKinds.includes(kind);
    input.addEventListener("change", () => {
      filters.hiddenKinds = input.checked ? filters.hiddenKinds.filter((other) => other !== kind) : [...filters.hiddenKinds, kind].sort();
      filtersChanged();
    });
    label.append(input, html("span", kind, "kind-name"), html("span", count, "kind-count"));
    box.append(label);
  }
  const present = new Set(kinds.map(([kind]) => kind));
  const active = [filters.violationsOnly, filters.idleOnly, !filters.outside, !filters.observed, !filters.manual].filter(Boolean).length
    + filters.hiddenKinds.filter((kind) => present.has(kind)).length;
  $("filters-count").hidden = !active;
  $("filters-count").textContent = active;
  $("filters-button").classList.toggle("active", active > 0);
  $("collapse-groups").hidden = !(scene && scene.open && scene.open.size);
}
function filtersChanged() {
  store("archgraph.filters.v1", filters);
  if (view === "diagram") redraw(); else updateFilterUI();
}
for (const [id, key] of [["filter-violations", "violationsOnly"], ["filter-idle", "idleOnly"], ["filter-outside", "outside"], ["filter-observed", "observed"], ["filter-manual", "manual"]]) {
  $(id).addEventListener("change", () => { filters[key] = $(id).checked; filtersChanged(); });
}
$("filters-reset").addEventListener("click", () => {
  Object.assign(filters, { violationsOnly: false, idleOnly: false, outside: true, observed: true, manual: true, hiddenKinds: [] });
  filtersChanged();
});
function togglePopover(button, panel, force) {
  const open = force ?? panel.hidden;
  panel.hidden = !open;
  button.setAttribute("aria-expanded", String(open));
}
function closePopovers() {
  togglePopover($("filters-button"), $("filters-panel"), false);
  togglePopover($("shortcuts-button"), $("shortcuts"), false);
}
$("filters-button").addEventListener("click", () => togglePopover($("filters-button"), $("filters-panel")));
$("shortcuts-button").addEventListener("click", () => togglePopover($("shortcuts-button"), $("shortcuts")));
document.addEventListener("click", (event) => {
  if (!$("search-form").contains(event.target)) closeSearch();
  if (!event.target.closest("#filters-panel, #filters-button")) togglePopover($("filters-button"), $("filters-panel"), false);
  if (!event.target.closest("#shortcuts, #shortcuts-button")) togglePopover($("shortcuts-button"), $("shortcuts"), false);
});
$("reset-layout").addEventListener("click", () => {
  if (!scene) return;
  const before = new Map([...scene.positions].map(([id, box]) => [id, { ...box }]));
  store(storageKey(layoutKind(), scene.focusId), null);
  redraw();
  settle(before, null);
});
$("collapse-groups").addEventListener("click", () => {
  if (!scene) return;
  const before = new Map([...scene.positions].map(([id, box]) => [id, { ...box }]));
  store(storageKey("groups", scene.focusId), null);
  redraw();
  settle(before, null);
});

// ---------------------------------------------------------------- wiring tools
const COLOUR_TITLES = { source: "Wires take their source's colour", kind: "Wires take their relation kind's colour", target: "Wires take their target's colour", none: "Wires in one colour" };
// The key to the active colouring. Hovering or focusing a row lights its
// wires; a source or target row selects its entry.
function drawLegend() {
  const legend = $("legend"), body = $("legend-body");
  body.replaceChildren();
  const colours = scene && scene.colours;
  legend.hidden = view !== "diagram" || !colours || !scene.edges.length;
  if (legend.hidden) return;
  $("legend-title").textContent = COLOUR_TITLES[colours.mode];
  if (scene.board && scene.board.fallback) body.append(html("p", scene.board.fallback, "legend-notice"));
  const row = (sample, text, lit, action, title, label) => {
    const button = html("button", null, "legend-row");
    button.type = "button";
    button.setAttribute("aria-label", label || `${text} wires`);
    button.append(sample, html("span", text, "legend-name"));
    if (title) button.title = title;
    const light = () => hoverCanvas(lit(), {});
    button.addEventListener("mouseenter", light);
    button.addEventListener("focus", light);
    button.addEventListener("mouseleave", () => hoverCanvas(null));
    button.addEventListener("blur", () => hoverCanvas(null));
    if (action) button.addEventListener("click", action);
    body.append(button);
  };
  const litBy = (test) => () => {
    const lit = { nodes: new Set(), edges: new Set() };
    for (const edge of scene.edges) if (test(edge)) { lit.edges.add(edge); lit.nodes.add(edge.from); lit.nodes.add(edge.to); }
    return lit;
  };
  if (colours.mode === "source" || colours.mode === "target") {
    const end = colours.mode === "source" ? "from" : "to";
    for (const [id, net] of colours.nets) {
      const node = scene.byId.get(id);
      if (!node) continue;
      row(wireSample({ net }), compact(entryTitle(node), 30), litBy((edge) => edge[end] === id), () => selectEntry(id, { centre: true }), displayEndpoint(id),
        `Wires ${end === "from" ? "from" : "into"} ${entryTitle(node)}`);
    }
  } else if (colours.mode === "kind") {
    for (const [kind, net] of colours.kinds) row(wireSample({ net }), kind, litBy((edge) => edge.parts.some((part) => part.kind === kind)));
    if (scene.edges.some((edge) => new Set(edge.parts.map((part) => part.kind)).size > 1)) {
      row(wireSample({ net: null, strands: [{ net: 0 }, { net: 1 }] }), "Mixed: a strand per kind", litBy((edge) => new Set(edge.parts.map((part) => part.kind)).size > 1));
    }
  }
  const marks = html("div", null, "legend-marks");
  body.append(marks);
  const mark = (sample, text, test) => { if (scene.edges.some(test)) { const item = html("span", null, "legend-mark"); item.append(sample, html("span", text)); marks.append(item); } };
  mark(wireSample({ net: null }, { violating: true }), "violation", (edge) => edge.violation_rule_ids.length > 0);
  mark(wireSample({ net: null }, { violating: true, cut: true }), "suggested cut", (edge) => edge.suggested_cut_rule_ids.length > 0);
  mark(wireSample({ net: null }, { manual: true }), "manual", (edge) => edge.origin === "manual");
}
function updateDisplayUI() {
  for (const mode of MODES) $(`mode-${mode}`).setAttribute("aria-pressed", String(display.mode === mode));
  $("colour-by").value = display.colour;
}
function setDisplay(change) {
  const modeChanged = change.mode && change.mode !== display.mode;
  Object.assign(display, change);
  store("archgraph.view.v1", display);
  updateDisplayUI();
  if (view !== "diagram" || !scene || !scene.positions) return;
  const before = new Map([...scene.positions].map(([id, box]) => [id, { ...box }]));
  redraw();
  if (modeChanged) { settle(before, null); animateTo(initialCamera()); }
}
for (const mode of MODES) $(`mode-${mode}`).addEventListener("click", () => setDisplay({ mode }));
$("colour-by").addEventListener("change", () => setDisplay({ colour: $("colour-by").value }));
updateDisplayUI();

// ---------------------------------------------------------------- navigation
async function loadFocus(id, pushHistory = true, options = {}) {
  const request = ++focusRequest;
  $("loading").hidden = false;
  $("error").hidden = true;
  const previous = currentProjection;
  // Opening a child: the camera dives into its card while the level loads.
  const child = view === "diagram" && scene && scene.positions ? scene.byId.get(`node:${id}`) : null;
  const entering = child && !child.outside_focus && !reducedMotion.matches ? child : null;
  let dive = null;
  if (entering) {
    $("graph").classList.add("leaving");
    dive = animateTo(cameraFor(scene.positions.get(entering.id), { maxK: 4, pad: -60 }), 300);
  }
  try {
    const [projection] = await Promise.all([api(`/api/focus/${encodeURIComponent(id)}`), dive]);
    if (request !== focusRequest) return;
    const sameFocus = previous !== null && previous.focus.id === projection.focus.id;
    const keep = { ...camera };
    currentProjection = projection;
    entries = new Map(projection.nodes.map((node) => [node.id, node]));
    merged = mergeEdges(projection.edges);
    if (!sameFocus) table.filter = "";
    if (pushHistory) history.pushState({ focus: id }, "", focusUrl(id));
    const trail = html("ol");
    for (const ancestor of projection.breadcrumbs.slice(0, -1)) {
      const item = html("li");
      item.append(linkTo(ancestor.id, ancestor.title));
      trail.append(item);
    }
    $("breadcrumbs").replaceChildren(trail);
    $("focus-title").textContent = projection.focus.title;
    $("focus-id").textContent = projection.focus.id;
    $("evidence-notice").textContent = projection.evidence_notice;
    document.title = `${projection.focus.title} · ArchGraph`;
    for (let at = projection.focus.id; at; at = tree.parents ? tree.parents.get(at) : null) tree.open.add(at);
    renderTree();
    renderViolationList();
    renderView();
    showOverview();
    if (view === "diagram") {
      const target = initialCamera();
      const back = previous && !sameFocus ? scene.byId.get(`node:${previous.focus.id}`) : null;
      if (sameFocus) setCamera(keep);
      else if (!previous || reducedMotion.matches) setCamera(target);
      else if (back) {
        // Going up: start inside the card of the level we came from.
        setCamera(cameraFor(scene.positions.get(back.id), { maxK: 4, pad: -60 }));
        animateTo(target, 380);
      } else if (entering) {
        const size = stageSize(), shrink = 0.6;
        setCamera({ k: target.k * shrink, x: size.width / 2 - (size.width / 2 - target.x) * shrink, y: size.height / 2 - (size.height / 2 - target.y) * shrink });
        animateTo(target, 320);
      } else setCamera(target);
    }
    if (options.select && scene && scene.byId.has(options.select)) selectEntry(options.select, { centre: true });
  } catch (error) { if (request === focusRequest) showError(error); }
  finally {
    if (request === focusRequest) { $("loading").hidden = true; $("graph").classList.remove("leaving"); }
  }
}
$("view-diagram").addEventListener("click", () => setView("diagram"));
$("view-table").addEventListener("click", () => setView("table"));
$("zoom-in").addEventListener("click", () => { const size = stageSize(); zoomAt(1.25, size.width / 2, size.height / 2); });
$("zoom-out").addEventListener("click", () => { const size = stageSize(); zoomAt(0.8, size.width / 2, size.height / 2); });
$("zoom-level").addEventListener("click", () => { const size = stageSize(); zoomAt(1 / camera.k, size.width / 2, size.height / 2); });
$("zoom-fit").addEventListener("click", fitView);
$("zoom-selection").addEventListener("click", zoomToSelection);
$("sidebar-toggle").addEventListener("click", () => togglePanel("sidebar"));
$("inspector-toggle").addEventListener("click", () => togglePanel("inspector"));
$("inspector-close").addEventListener("click", () => togglePanel("inspector", false));
$("scrim").addEventListener("click", () => togglePanel(drawer, false));
$("overview-button").addEventListener("click", showOverview);
$("notes-button").addEventListener("click", () => {
  togglePanel("inspector", true);
  showOverview();
  $("diagnostics-section").open = true;
  $("diagnostics-section").scrollIntoView({ block: "start" });
});
narrow.addEventListener("change", () => { drawer = null; applyPanels(); });
window.addEventListener("resize", () => { if (scene) applyCamera(); });
window.addEventListener("popstate", () => { if (meta) loadFocus(new URL(location.href).searchParams.get("focus") || meta.project.root, false); });
applyPanels();
(async () => {
  try {
    meta = await api("/api/meta");
    showNotesButton();
    showRefreshState(false);
    await loadTree();
    await loadFocus(new URL(location.href).searchParams.get("focus") || meta.project.root, false);
    if (meta.watching) setInterval(() => { if (!document.hidden) checkForUpdates(); }, 5000);
  } catch (error) { showError(error); }
})();
