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
// Above this many drawn edges, focus mode (on by default) draws every wire
// faint until an entry, a wire or a legend row is pointed at or selected;
// violations stay strong.
const FOCUS_EDGE_LIMIT = 60;
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
  for (const previous of document.querySelectorAll("#graph .selected, #table-wrap .selected, #dsm-wrap .selected, #violations .selected, #details .selected")) previous.classList.remove("selected");
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
    if (!item.marker) continue;
    const strong = item.element.classList.contains("lit") || item.element.classList.contains("hover");
    item.line.setAttribute("marker-end", strong ? item.litMarker : item.marker);
  }
  // A trunk lights with any of its wires; lit for only some of them, its
  // body dims and their own strands show through it. A selection's overlay
  // stays under a hover's.
  for (const item of scene.trunkEls || []) {
    const count = lit ? item.trunk.members.filter((edge) => lit.edges.has(edge)).length : 0;
    const partial = count > 0 && count < item.trunk.members.length;
    for (const element of [item.element, item.tagGroup]) {
      element.classList.toggle(className, count > 0);
      element.classList.toggle(`${className}-partial`, partial);
    }
    const own = className === "hover" ? (partial ? lit.edges : null) : null;
    const chosen = selection ? item.trunk.members.filter((edge) => selection.edges.has(edge)).length : 0;
    overlayTrunk(item, own || (chosen > 0 && chosen < item.trunk.members.length ? selection.edges : null));
  }
}
// Hovering an entry or edge on the canvas marks the rows naming it in the
// details panel; hovering a row lights its edge and entry on the canvas.
function linkPanel(source) {
  for (const row of document.querySelectorAll("#details .dependency")) {
    row.classList.toggle("linked", Boolean(source) && (source.trunk ? source.trunk.members.includes(row.edge) : source.edge ? row.edge === source.edge : row.otherId === source.node));
  }
}
function hoverCanvas(lit, source) {
  paint(lit, "hover", "has-hover");
  linkPanel(lit ? source : null);
}
// A trunk's details: its wires, each opening its own evidence.
function showTrunk(trunk, element) {
  select(element, trunkScope(trunk));
  selected = { kind: "trunk", id: trunk.id };
  const target = scene.byId.get(trunk.target);
  const violations = trunk.members.filter(violates);
  const panel = detailsTitle(trunkText(trunk), displayEndpoint(trunk.target), "Trunk");
  const sources = new Set(trunk.members.map((edge) => edge.from)).size;
  panel.append(html("p", `${plural(trunk.members.length, "wire")} from ${sources} ${sources === 1 ? "entry" : "entries"} into ${target ? entryTitle(target) : trunk.target}, drawn as one trunk that fans out near their sources.`));
  if (violations.length) {
    const rules = [...new Set(violations.flatMap((edge) => edge.violation_rule_ids))].sort();
    panel.append(html("p", `⚠ ${violations.length} of them ${violations.length === 1 ? "violates" : "violate"} ${rules.join(", ")}`, "violation-badge"));
  }
  const actions = html("div", null, "actions");
  if (target) actions.append(actionButton(`Show ${entryTitle(target)}`, () => selectEntry(trunk.target, { centre: true }), "secondary"));
  panel.append(actions);
  dependencyList(panel, "Wires in this trunk", trunk.members, (edge) => edge.from);
  revealDetails();
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
    button.addEventListener("click", () => showEdge(edge, drawnEdge(edge)));
    const item = html("li");
    item.append(button);
    list.append(item);
  }
  panel.append(list);
  if (edges.length > limit) panel.append(html("p", `${edges.length - limit} more not listed.`, "notice"));
}
// Where an edge is drawn in the current view: its wire, or its matrix cell.
function drawnEdge(edge) {
  if (scene && scene.dsm) return scene.dsm.cellFor(edge);
  const drawn = scene && scene.edgeEls && scene.edgeEls.find((item) => item.edge === edge);
  return drawn ? drawn.element : null;
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
  } else if (selected.kind === "trunk") {
    const item = (scene.trunkEls || []).find(({ trunk }) => trunk.id === selected.id);
    if (item) select(item.element, trunkScope(item.trunk)); else showOverview();
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
  const before = new Map([...(scene.positions || [])].map(([id, box]) => [id, { ...box }]));
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

// ---------------------------------------------------------------- trunks
// Wires from several sibling entries (those inside the focus, or those
// outside it) into the same target merge into one trunk, like a cable
// harness: each source keeps a short branch, and the trunk carries them all
// to the target, labelled with their number. It takes at least TRUNK_MIN
// wires heading the same way; manual dependencies are never merged.
const TRUNK_MIN = 3;
function trunkKey(edge) {
  if (edge.origin === "manual") return null;
  const source = scene.byId.get(edge.from);
  return `${edge.to}\u0000${source && source.outside_focus ? "outside" : "inside"}`;
}
// A trunk is drawn as a ribbon cable: a strand per colour among its wires
// (per source colour, or per relation kind), side by side, and each wire's
// branch runs into its own strand. At most Board.TRUNK_STRANDS strands: past
// that, sources that share a hue share its strand (drawn solid, without
// their dash patterns), and if that is still too many the last strand, in
// grey, stands for the rest. Coloured by target (or not at all) a trunk is
// one wide strand.
function strandKeys(edge) {
  const colours = scene.colours;
  if (!colours || colours.mode === "none" || colours.mode === "target") return ["one"];
  if (colours.mode === "kind") return kindCounts(edge.parts).map(([kind]) => `k:${kind}`);
  const net = colours.nets.get(edge.from);
  return [net === undefined ? "plain" : `n:${net}`];
}
function strandClass(key, members) {
  const colours = scene.colours;
  if (key === "more") return "more";
  if (key === "plain") return "";
  if (key === "one") {
    const look = edgeLook(members[0]);
    return colours && colours.mode === "target" && look.net !== null && look.net !== undefined ? netClass(look.net) : "";
  }
  if (key.startsWith("h:")) return `n${key.slice(2)}`;
  if (key.startsWith("k:")) { const net = colours.kinds.get(key.slice(2)); return net === undefined ? "" : netClass(net); }
  return netClass(Number(key.slice(2)));
}
function ribbonOf(members) {
  const limit = Board.TRUNK_STRANDS;
  const rank = (key) => (/^[nh]:/.test(key) ? Number(key.slice(2)) : key.startsWith("k:") ? scene.colours.kinds.get(key.slice(2)) ?? 1e6 : 1e7);
  const distinct = (map) => [...new Set(members.flatMap((edge) => strandKeys(edge).map(map)))].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
  let merge = (key) => key, keys = distinct(merge);
  const merged = keys.length > limit;
  if (merged) { merge = (key) => (key.startsWith("n:") ? `h:${Number(key.slice(2)) % PALETTE}` : key); keys = distinct(merge); }
  const kept = keys.length > limit ? keys.slice(0, limit - 1) : keys;
  const more = keys.length - kept.length;
  const strands = kept.map((key) => ({ key, className: strandClass(key, members) }));
  if (more) strands.push({ key: "more", className: "more" });
  const slot = new Map(strands.map((strand, i) => [strand.key, i]));
  const slots = new Map(members.map((edge) => [edge, [...new Set(strandKeys(edge).map((key) => { const at = slot.get(merge(key)); return at === undefined ? slot.get("more") : at; }))].sort((a, b) => a - b)]));
  return { strands, slots, more, merged, wide: strands.length === 1 };
}
function trunkText(trunk) {
  const target = scene.byId.get(trunk.target);
  return `×${trunk.members.length} → ${compact(target ? entryTitle(target) : trunk.target, 30)}`;
}
// Trunk geometry: a tail is a polyline (boards) or a chain of cubic curves.
const unit = (p, q) => { const d = Math.hypot(q.x - p.x, q.y - p.y) || 1; return { x: (q.x - p.x) / d, y: (q.y - p.y) / d }; };
function tailEnd(tail) {
  if (tail.points) { const n = tail.points.length; return { tip: tail.points[n - 1], dir: unit(tail.points[Math.max(0, n - 2)], tail.points[n - 1]) }; }
  const last = tail.curves[tail.curves.length - 1];
  return { tip: last[3], dir: unit(Math.hypot(last[3].x - last[2].x, last[3].y - last[2].y) > 0.5 ? last[2] : last[0], last[3]) };
}
// The tail without its last `length` (where the arrowhead sits).
function trimTail(tail, length) {
  if (tail.points) {
    const points = tail.points.map((p) => ({ ...p }));
    if (points.length < 2) return { points };
    let left = length;
    while (points.length > 2 && Math.hypot(points[points.length - 1].x - points[points.length - 2].x, points[points.length - 1].y - points[points.length - 2].y) <= left) {
      left -= Math.hypot(points[points.length - 1].x - points[points.length - 2].x, points[points.length - 1].y - points[points.length - 2].y);
      points.pop();
    }
    const n = points.length, d = unit(points[n - 2] || points[n - 1], points[n - 1]);
    if (n >= 2) points[n - 1] = { x: points[n - 1].x - d.x * left, y: points[n - 1].y - d.y * left };
    return { points };
  }
  const curves = tail.curves.map((curve) => curve.map((p) => ({ ...p })));
  const last = curves[curves.length - 1], d = unit(Math.hypot(last[3].x - last[2].x, last[3].y - last[2].y) > 0.5 ? last[2] : last[0], last[3]);
  last[3] = { x: last[3].x - d.x * length, y: last[3].y - d.y * length };
  last[2] = { x: last[2].x - d.x * length, y: last[2].y - d.y * length };
  return { curves };
}
function tailPath(tail, offset) {
  if (tail.points && tail.points.length < 2) return "";
  return tail.points ? Board.pathData(Board.offsetPolyline(tail.points, offset)) : offsetCurves(tail.curves, offset);
}
function tailPolyline(tail) {
  if (tail.points) return tail.points;
  const points = [tail.curves[0][0]];
  for (const curve of tail.curves) { if (Math.hypot(curve[0].x - points[points.length - 1].x, curve[0].y - points[points.length - 1].y) > 0.5) points.push(curve[0]); for (let i = 1; i <= 12; i++) points.push(bezier(curve, i / 12)); }
  return points;
}
const tailLength = (tail) => { const points = tailPolyline(tail); let length = 0; for (let i = 1; i < points.length; i++) length += Math.hypot(points[i].x - points[i - 1].x, points[i].y - points[i - 1].y); return length; };

// ---------------------------------------------------------------- routing
// Downward edges leave a card's bottom and enter the next card's top; upward
// ones (against the layer order) run top to bottom and bow to the side, so
// they stand out. Each card side spreads its edges over several ports.
// Edges of entries the user moved take a direct curve instead. A trunk's
// wires run to a junction just past their own row; from junction to
// junction, row by row, the trunk gathers them and carries them on to the
// target, each wire joining its strand of the ribbon.
function route(edges, positions, moved) {
  const plans = edges.map((edge, order) => {
    const a = positions.get(edge.from), b = positions.get(edge.to);
    if (!a || !b) throw new Error("Projection edge references an absent visible node");
    return { edge, order, a, b, fromId: edge.from, toId: edge.to, free: moved.has(edge.from) || moved.has(edge.to), dir: b.row > a.row ? "down" : b.row < a.row ? "up" : "same" };
  });
  const groups = new Map(), trunks = [];
  for (const plan of plans) {
    const key = plan.free || plan.dir === "same" ? null : trunkKey(plan.edge);
    if (key) { if (!groups.has(`${key}\u0000${plan.dir}`)) groups.set(`${key}\u0000${plan.dir}`, []); groups.get(`${key}\u0000${plan.dir}`).push(plan); }
  }
  let order = plans.length;
  for (const [id, members] of groups) {
    if (members.length < TRUNK_MIN) continue;
    const down = members[0].dir === "down", target = members[0].b, targetId = members[0].toId;
    const byRow = new Map();
    for (const plan of members) { if (!byRow.has(plan.a.row)) byRow.set(plan.a.row, []); byRow.get(plan.a.row).push(plan); }
    const tx = target.x + target.width / 2;
    const junctions = [], carriers = [], start = new Map();
    let joined = 0, previous = null;
    for (const row of [...byRow.keys()].sort((a, b) => (down ? a - b : b - a))) {
      const riders = byRow.get(row), box = riders[0].a;
      const mean = riders.reduce((sum, plan) => sum + plan.a.x + plan.a.width / 2, 0) / riders.length;
      const junction = { x: previous ? (previous.x + mean + tx) / 3 : (2 * mean + tx) / 3, y: down ? box.y + box.height + GAP_Y * 0.42 : box.y - GAP_Y * 0.42,
        width: 0, height: 0, row: row + (down ? 0.5 : -0.5) };
      for (const plan of riders) { plan.b = junction; plan.toId = null; plan.trunk = id; start.set(plan.edge, junctions.length); }
      if (previous) carriers.push({ edge: null, trunk: id, order: order++, a: previous, b: junction, fromId: null, toId: null, free: false, dir: members[0].dir, count: joined });
      joined += riders.length;
      junctions.push(junction);
      previous = junction;
    }
    carriers.push({ edge: null, trunk: id, order: order++, a: previous, b: target, fromId: null, toId: targetId, free: false, dir: members[0].dir, count: joined, last: true });
    trunks.push({ id, target: targetId, members: members.map((plan) => plan.edge), carriers, carrier: carriers[carriers.length - 1], junctions, start });
  }
  const all = [...plans, ...trunks.flatMap((trunk) => trunk.carriers)];
  const ports = new Map();
  const attach = (id, box, side, plan, end, other) => {
    const key = `${id}\u0000${side}`;
    if (!ports.has(key)) ports.set(key, { box, side, items: [] });
    ports.get(key).items.push({ plan, end, other });
  };
  for (const plan of all) {
    if (plan.free) continue;
    if (plan.fromId) attach(plan.fromId, plan.a, plan.dir === "down" ? "bottom" : "top", plan, "start", plan.b);
    else plan.start = { x: plan.a.x, y: plan.a.y };
    if (plan.toId) attach(plan.toId, plan.b, plan.dir === "up" ? "bottom" : "top", plan, "end", plan.a);
    else plan.end = { x: plan.b.x, y: plan.b.y };
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
  const between = (from, to) => {
    const list = [];
    if (to > from) for (let row = Math.floor(from) + 1; row < to; row++) list.push(row);
    else for (let row = Math.ceil(from) - 1; row > to; row--) list.push(row);
    return list;
  };
  for (const plan of all) {
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
    for (const row of between(plan.a.row, plan.b.row)) {
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
  return { plans, trunks };
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
  const weight = (plan) => (plan.edge ? (plan.edge.violation_rule_ids.length ? 1e9 : 0) + plan.edge.count : 1e12);
  const order = plans.filter((plan) => !plan.trunk).sort((a, b) => weight(b) - weight(a) || a.order - b.order);
  for (const plan of plans) if (plan.trunk) { plan.crowded = true; plan.label = bezier(plan.curves[0], 0.5); }
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
// glyph in the card's corner. Widths come from a canvas measuring the text in
// the element's own font: no layout, so a thousand cards fit quickly and
// again whenever the zoom changes their size. The font is read once the
// element is in the document, so drawNode leaves the fitting for afterwards.
const measureFonts = new Map();
let measureContext = null;
function fontOf(element) {
  const key = element.getAttribute("class") || "";
  if (measureFonts.has(key)) return measureFonts.get(key);
  const style = getComputedStyle(element);
  const font = style.fontFamily ? { spec: `${style.fontStyle} ${style.fontWeight} 100px ${style.fontFamily}`, size: parseFloat(style.fontSize) || 14 } : null;
  if (font) measureFonts.set(key, font);
  return font;
}
// The drawn width of `text` in the element's font, at `size` px or the
// element's own size; null when the font is unknown (not in the document).
function textWidth(element, text, size) {
  const font = fontOf(element);
  if (!font) return null;
  measureContext ||= document.createElement("canvas").getContext("2d");
  if (measureContext.font !== font.spec) measureContext.font = font.spec;
  // A small margin: canvas and SVG may round glyph advances differently.
  return measureContext.measureText(text).width * (size || font.size) / 100 * 1.02;
}
// The longest cut of `full` (at its end, or its start for paths) that fits.
function fitLine(element, full, width, fromStart, size) {
  const cut = (length) => (fromStart ? compactStart(full, length) : compact(full, length));
  const fits = (text) => textWidth(element, text, size) <= width;
  if (textWidth(element, full, size) === null) return cut(fromStart ? 30 : 26);
  if (fits(full)) return full;
  let low = 2, high = full.length - 1;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (fits(cut(middle))) low = middle; else high = middle - 1;
  }
  return cut(low);
}
function fitText(element, full, width, fromStart) {
  element.textContent = fitLine(element, full, width, fromStart);
}
// A title on up to two lines. It breaks after a separator (or before a
// capital), leaving at least three characters on each line: where both
// lines fit, at the most even such break; otherwise at the last one whose
// first line fits, with the second cut to fit. Without such a break it
// stays on one line, cut to fit.
function twoLines(element, full, width, size) {
  const fits = (text) => { const measured = textWidth(element, text, size); return measured === null || measured <= width; };
  if (fits(full)) return [full];
  const breaks = [];
  for (let at = 3; at <= full.length - 3; at++) {
    if (/[_./\-\s]/.test(full[at - 1]) || (/[a-z0-9]/.test(full[at - 1]) && /[A-Z]/.test(full[at]))) breaks.push(at);
  }
  const both = breaks.filter((at) => fits(full.slice(0, at)) && fits(full.slice(at)));
  if (both.length) {
    const at = both.reduce((best, at) => (Math.min(at, full.length - at) > Math.min(best, full.length - best) ? at : best));
    return [full.slice(0, at), full.slice(at)];
  }
  const at = breaks.filter((at) => fits(full.slice(0, at))).pop();
  if (at) return [full.slice(0, at), fitLine(element, full.slice(at), width, false, size)];
  return [fitLine(element, full, width, false, size)];
}

// ---------------------------------------------------------------- text size
// Card titles and wire labels keep a readable size on screen. Below 100%
// they grow as the view zooms out, in steps (each step refits the titles),
// up to a size set by the card: 22 px on a full card; below COMPACT_ZOOM a
// card shows only its title, on up to two lines of up to 32 px. Past those
// caps the text shrinks with the card. Wire labels grow likewise and hide
// in compact cards' zoom range except on the highlighted wires. A trunk's
// full tag stays down to TAG_ZOOM; below it, the tag shows only the count,
// which keeps growing (up to TAG_SHORT_MAX times) so it stays readable.
const TEXT = { title: 17, label: 14, minTitle: 12, minLabel: 11, maxTitle: 22, maxCompactTitle: 32, maxLabel: 20 };
const COMPACT_ZOOM = 0.6, TAG_ZOOM = 0.2, TAG_SCALE = 1.8, TAG_SHORT_MAX = 8;
const textStep = (size, base) => base * Math.pow(1.1, Math.max(0, Math.ceil(Math.log(size / base) / Math.log(1.1) - 1e-9)));
function textTier(k) {
  const compact = k < COMPACT_ZOOM;
  const want = TEXT.minTitle / k;
  const title = compact ? Math.min(TEXT.maxCompactTitle, textStep(want, TEXT.title)) : Math.min(TEXT.maxTitle, textStep(Math.max(want, TEXT.title), TEXT.title));
  const label = Math.min(TEXT.maxLabel, textStep(Math.max(TEXT.minLabel / k, TEXT.label), TEXT.label));
  const tag = Math.min(TAG_SCALE, textStep(Math.max(TEXT.minTitle / k, TEXT.label), TEXT.label) / TEXT.label);
  const tagShort = Math.min(TAG_SHORT_MAX, textStep(Math.max(TEXT.minTitle / k, TEXT.label), TEXT.label) / TEXT.label);
  const up = (value, digits) => Math.ceil(value * digits - 1e-6) / digits;
  return { compact, title: up(title, 10), label: up(label / TEXT.label, 100), tag: up(tag, 100), tagShort: up(tagShort, 100), key: `${compact}:${up(title, 10)}` };
}
function updateText() {
  if (!scene || !scene.cards) return;
  updateWires();
  const tier = textTier(camera.k), graph = $("graph");
  graph.style.setProperty("--label-scale", String(tier.label));
  graph.style.setProperty("--tag-scale", String(tier.tag));
  graph.style.setProperty("--tag-short-scale", String(tier.tagShort));
  graph.classList.toggle("compact", tier.compact);
  if (scene.textKey === tier.key) return;
  scene.textKey = tier.key;
  graph.style.setProperty("--title-size", `${tier.title}px`);
  for (const card of scene.cards) fitTitle(card, tier);
}
function fitTitle(card, tier) {
  const { element, full, box } = card;
  if (!tier.compact) {
    element.textContent = fitLine(element, full, card.width, false, tier.title);
    element.setAttribute("y", card.y);
    return;
  }
  const lines = twoLines(element, full, card.compactWidth, tier.title);
  const lead = tier.title * 1.08;
  const first = box.height / 2 - (lines.length - 1) * lead / 2 + tier.title * 0.35;
  element.setAttribute("y", round(first));
  element.replaceChildren(...lines.map((line, i) => svg("tspan", { x: 14, y: round(first + i * lead) }, line)));
}
// ---------------------------------------------------------------- wires
// How the canvas draws wires: the routing mode (curves, a PCB board with
// 0°/45°/90° traces, or a hexagonal board with 0°/60°/120° traces) and what
// gives a wire its colour. Remembered in the browser like the other views.
const MODES = ["curves", "pcb", "hex"], COLOURINGS = ["source", "kind", "target", "none"];
const display = (() => {
  const saved = stored("archgraph.view.v1", {}) || {};
  return { mode: MODES.includes(saved.mode) ? saved.mode : "curves", colour: COLOURINGS.includes(saved.colour) ? saved.colour : "source",
    focus: typeof saved.focus === "boolean" ? saved.focus : true };
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
  let positions, notes, left, plans, trunks = [];
  Object.assign(scene, { board: null, frameTraces: null, dropCell: null, ghost: null });
  if (board) {
    const fixed = new Map();
    for (const [id, place] of Object.entries(saved)) if (Array.isArray(place) && place.length === 2 && place.every(Number.isInteger)) fixed.set(id, place);
    const started = performance.now();
    const result = Board.route({ mode: display.mode, rows: arrangement.rows, centre: arrangement.centre, unit: CARD.width + GAP_X, card: CARD, fixed,
      edges: scene.edges.map((edge) => ({ from: edge.from, to: edge.to, halfWidth: busHalfWidth(edge), trunk: trunkKey(edge), strands: strandKeys(edge) })), trunkMin: TRUNK_MIN,
      labels: scene.edges.length <= LABEL_LIMIT ? scene.edges.map((edge) => ({ width: labelText(edge).length * 6.6 + 10, height: 16,
        priority: (edge.violation_rule_ids.length ? 1e9 : 0) + edge.count })) : null });
    scene.routeTime = performance.now() - started;
    ({ positions, notes } = result);
    left = PAD + 24;
    scene.board = result;
    // A trunk's wire is drawn up to where it joins the ribbon.
    const branches = new Map();
    for (const trunk of result.trunks) for (const part of trunk.parts) branches.set(part.ti, part.prefix);
    plans = result.traces.map((trace, order) => {
      const points = branches.has(order) ? branches.get(order) : trace.points;
      return { edge: scene.edges[order], order, dir: trace.dir, points, core: trace.core,
        d: Board.pathData(points), label: trace.label, crowded: trace.label.crowded, trunk: trace.trunk };
    });
    trunks = result.trunks.map((trunk) => ({ id: trunk.id, target: scene.edges[trunk.members[0]].to, members: trunk.members.map((ti) => scene.edges[ti]),
      tails: new Map(trunk.parts.map((part) => [scene.edges[part.ti], { points: part.tail }])) }));
    const inside = new Set(scene.entries.filter((node) => !node.outside_focus).map((node) => node.id));
    scene.frameTraces = plans.filter((plan) => inside.has(plan.edge.from) && inside.has(plan.edge.to)).flatMap((plan) => plan.points);
  } else {
    ({ positions, notes, left } = layout(arrangement));
    for (const [id, box] of positions) {
      const place = saved[id];
      if (Array.isArray(place) && place.length === 2 && place.every(Number.isFinite)) { box.x = place[0]; box.y = place[1]; moved.add(id); }
    }
  }
  Object.assign(scene, { positions, moved, nodeEls: new Map(), edgeEls: [], trunkEls: [], cards: [], textKey: null });
  graph.setAttribute("class", `mode-${display.mode} colour-${display.colour}${scene.edges.length > LABEL_LIMIT ? " quiet" : ""}${display.focus && scene.edges.length > FOCUS_EDGE_LIMIT ? " faint" : ""}${filters.violationsOnly ? " violations-only" : ""}${filters.idleOnly ? " idle-only" : ""}`);
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
    const routed = route(scene.edges, positions, moved);
    plans = routed.plans;
    for (const plan of plans) plan.text = labelText(plan.edge);
    if (scene.edges.length <= LABEL_LIMIT) placeLabels(plans, positions);
    else for (const plan of plans) plan.label = bezier(plan.curves[0], 0.5);
    trunks = routed.trunks.map((trunk) => ({ id: trunk.id, target: trunk.target, members: trunk.members,
      tails: new Map(trunk.members.map((edge) => [edge, { curves: trunk.carriers.slice(trunk.start.get(edge)).flatMap((carrier) => carrier.curves) }])) }));
  }
  for (const plan of plans) plan.text = labelText(plan.edge);
  scene.trunks = trunks;
  // Violating edges are drawn last, on top of the others.
  plans.sort((a, b) => (a.edge.violation_rule_ids.length > 0) - (b.edge.violation_rule_ids.length > 0) || a.order - b.order);
  for (const plan of plans) graph.append(drawEdge(plan));
  // Trunks go over the wires they carry, violating ones last.
  const tags = [];
  for (const trunk of [...trunks].sort((a, b) => a.members.some(violates) - b.members.some(violates) || a.id.localeCompare(b.id))) graph.append(drawTrunk(trunk, tags));
  for (const item of scene.trunkEls) graph.append(item.tagGroup);
  for (const fit of tags) fit();
  scene.wireKey = null;
  scene.entries.forEach((node, index) => {
    const element = drawNode(node, positions.get(node.id), index === 0);
    graph.append(element);
    for (const fit of element.fits) fitText(...fit);
  });
  updateText();
  drawLegend();
  updateDisplayUI();
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
  const group = svg("g", { class: `edge ${edge.origin} w${weight}${net}${look.strands ? " bus" : ""}${plan.trunk ? " branch" : ""}${plan.dir === "up" ? " upward" : ""}${plan.crowded ? " crowded" : ""}${violating ? " violating" : ""}${cut ? " cut" : ""}`, "aria-hidden": "true", "data-from": edge.from, "data-to": edge.to });
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
  // A trunk's wires end in the trunk's own arrowhead.
  const line = svg("path", { d: plan.d, class: "edge-line" });
  if (!plan.trunk) line.setAttribute("marker-end", `url(#${ids.end})`);
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
  const marker = plan.trunk ? null : `url(#${ids.end})`;
  // Without colours a highlighted wire turns to ink; with them it keeps its colour.
  const litMarker = marker && !violating && (display.colour === "none" || look.strands) ? "url(#arrow-lit)" : marker;
  scene.edgeEls.push({ edge, core: plan.core || null, trunk: plan.trunk || null, element: group, line, hit, casing, gap, strands, label, marker, litMarker, violating, count: look.strands ? look.strands.length : 0 });
  return group;
}
const violates = (edge) => edge.violation_rule_ids.length > 0;
// A trunk: a ribbon cable of strands (one per colour among its wires, see
// ribbonOf) over a casing, an arrowhead as wide as the ribbon at the target,
// chevrons along it pointing the way, and a tag beside the arrowhead with
// the count and the target. A violation among its wires puts the red casing
// around the whole ribbon and a count on the tag. Strands, arrowhead and
// chevrons are laid out again when the zoom changes (layoutTrunk): the
// ribbon keeps a minimum width on screen, the chevrons a fixed spacing.
function drawTrunk(trunk, tags) {
  const violations = trunk.members.filter(violates).length;
  const ribbon = ribbonOf(trunk.members);
  const single = ribbon.wide && ribbon.strands[0].className;
  const group = svg("g", { class: `edge trunk${single ? ` ${single}` : ribbon.wide ? " neutral" : " ribbon"}${violations ? " violating" : ""}`, "aria-hidden": "true", "data-trunk": trunk.id });
  // The strands overlap where tails share a course: dimmed as one group,
  // they fade evenly.
  const body = svg("g", { class: "trunk-body" });
  const casings = svg("g", { class: "trunk-casings" }), strands = svg("g", { class: "trunk-strands" });
  const arrow = svg("path", { class: "trunk-arrow" }), chevrons = svg("g", { class: "trunk-chevrons" });
  body.append(casings, strands, arrow, chevrons);
  // Lit for only some of its wires, the body dims and copies of their
  // strands show their own course through it.
  const overlay = svg("g", { class: "trunk-overlay" });
  group.append(body, overlay);
  const casingPaths = [], strandPaths = [];
  for (const [edge] of trunk.tails) {
    const hit = svg("path", { class: "edge-hit trunk-hit" });
    const casing = svg("path", { class: violations ? "edge-casing trunk-casing" : "edge-gap trunk-casing" });
    casingPaths.push({ edge, hit, element: casing });
    casings.append(hit, casing);
    for (const slot of ribbon.slots.get(edge)) {
      const strand = ribbon.strands[slot];
      const path = svg("path", { class: `trunk-strand${ribbon.wide ? " wide" : ""}${strand.className ? ` ${strand.className}` : ""}` });
      strandPaths.push({ edge, slot, element: path });
      strands.append(path);
    }
  }
  // Chevrons run along the longest tail.
  let spine = null, spineLength = -1;
  for (const tail of trunk.tails.values()) { const length = tailLength(tail); if (length > spineLength) { spine = tail; spineLength = length; } }
  const item = { trunk, element: group, body, ribbon, violations, casingPaths, strandPaths, arrow, chevrons, overlay, spine, spineLength, shapes: [], lit: null };
  // The tag: the count and the target; zoomed far out only the count.
  const tag = svg("g", { class: "trunk-tag" });
  const variant = (className, content, strip) => {
    const scale = svg("g", { class: className });
    const plate = svg("rect", { class: "tag-plate", height: 22, rx: 2 });
    const text = svg("text", { class: "tag-text" }, content);
    if (violations) text.append(svg("tspan", { class: "tag-alert" }, `  ⚠ ${violations}`));
    scale.append(plate, ...strip, text);
    tag.append(scale);
    const shape = { scale, plate, text, strip, width: 0 };
    item.shapes.push(shape);
    tags.push(() => {
      shape.width = (textWidth(text, text.textContent) || text.textContent.length * 7.4) + 16 + strip.length * 5;
      placeTag(item);
    });
  };
  // A ribbon's tag shows its colours; a merged last strand says how many.
  const strip = ribbon.wide ? [] : ribbon.strands.map((strand) => svg("rect", { class: `tag-net ${strand.className}`, width: 4, height: 14 }));
  variant("tag-full", `${trunkText(trunk)}${ribbon.more ? `, ${ribbon.more + 1} more colours in grey` : ""}`, strip);
  variant("tag-short", `×${trunk.members.length}`, []);
  // Tags go in a layer over every trunk (drawScene appends it last), in a
  // group of their own that lights and dims with the trunk.
  const tagGroup = svg("g", { class: group.getAttribute("class"), "aria-hidden": "true", "data-trunk-tag": trunk.id });
  tagGroup.append(tag);
  Object.assign(item, { tag, tagGroup });
  const sources = trunk.members.map((edge) => displayEndpoint(edge.from));
  group.append(svg("title", {}, `${trunkText(trunk)}${violations ? `, ${plural(violations, "violation")}` : ""}\n${sources.slice(0, 12).join("\n")}${sources.length > 12 ? `\n… and ${sources.length - 12} more` : ""}\nClick to list its wires.`));
  const lit = trunkScope(trunk);
  for (const element of [group, tagGroup]) {
    element.addEventListener("click", () => showTrunk(trunk, group));
    element.addEventListener("mouseenter", () => { if (!gesture) hoverCanvas(lit, { trunk }); });
    element.addEventListener("mouseleave", () => { if (!gesture) hoverCanvas(null); });
  }
  scene.trunkEls.push(item);
  return group;
}
// Chevrons along a trunk are CHEVRON_PX apart on screen.
const CHEVRON_PX = 150;
// Lays a trunk out at zoom `k`. A strand keeps STRAND_MIN_PX on screen, a
// little less in a ribbon of many (at most RIBBON_MAX_PX in all), and a
// single wide strand WIDE_MIN_PX.
const STRAND_MIN_PX = 3, RIBBON_MAX_PX = 12, WIDE_MIN_PX = 4;
function layoutTrunk(item, k) {
  const { ribbon, trunk } = item;
  const nominal = ribbon.wide ? Board.WIDE : Board.STRAND;
  const least = ribbon.wide ? WIDE_MIN_PX : Math.max(2, Math.min(STRAND_MIN_PX, RIBBON_MAX_PX / ribbon.strands.length));
  const scale = Math.min(12, Math.max(1, least / (nominal * k)));
  const strandWidth = nominal * scale;
  const width = strandWidth * ribbon.strands.length;
  const half = width / 2 + 2.5 * scale, length = Math.max(half * 2.5, 9 * scale);
  const offset = (slot) => (slot - (ribbon.strands.length - 1) / 2) * strandWidth;
  const trimmed = new Map([...trunk.tails].map(([edge, tail]) => [edge, trimTail(tail, length * 0.7)]));
  const seen = new Set();
  for (const { edge, hit, element } of item.casingPaths) {
    const d = tailPath(trimmed.get(edge), 0);
    // Tails share their last stretch: one casing per distinct course.
    element.setAttribute("d", seen.has(d) ? "" : d);
    hit.setAttribute("d", seen.has(d) ? "" : d);
    seen.add(d);
    // The casing's border keeps about 2.5 px (a gap 1 px) on screen.
    element.style.strokeWidth = `${round(width + (item.violations ? Math.max(7, 5 / k) : Math.max(3, 2 / k)))}px`;
    hit.style.strokeWidth = `${round(Math.max(width + 8 * scale, 16))}px`;
  }
  const paths = item.strandPaths.map(({ edge, slot }) => tailPath(trimmed.get(edge), offset(slot)));
  item.strandPaths.forEach(({ element }, i) => { element.setAttribute("d", paths[i]); element.style.strokeWidth = `${round(strandWidth * 1.08)}px`; });
  item.paths = paths;
  const { tip, dir } = tailEnd(item.spine);
  const normal = { x: -dir.y, y: dir.x };
  const base = { x: tip.x - dir.x * length, y: tip.y - dir.y * length };
  const corners = [tip, { x: base.x + normal.x * half, y: base.y + normal.y * half }, { x: base.x - normal.x * half, y: base.y - normal.y * half }];
  item.arrow.setAttribute("d", `M ${corners.map((p) => `${round(p.x)} ${round(p.y)}`).join(" L ")} Z`);
  item.arrow.style.strokeWidth = `${round(scale)}px`;
  item.end = { tip, dir, corners };
  // Chevrons: from the arrowhead back along the spine, where it runs
  // straight (a board's corner or a sharp bend would bend them).
  const chevrons = [];
  if (item.spine) {
    const points = tailPolyline(item.spine).slice().reverse(), spacing = CHEVRON_PX / k, size = width / 2 + 2 * scale;
    const at = [0];
    for (let i = 1; i < points.length; i++) at.push(at[i - 1] + Math.hypot(points[i].x - points[i - 1].x, points[i].y - points[i - 1].y));
    // The point `distance` back from the tip, and the direction of travel.
    const sample = (distance) => {
      let i = 1;
      while (i < points.length - 1 && at[i] < distance) i++;
      const p = points[i - 1], q = points[i], t = (distance - at[i - 1]) / ((at[i] - at[i - 1]) || 1);
      return { x: p.x + (q.x - p.x) * t, y: p.y + (q.y - p.y) * t, u: unit(q, p) };
    };
    const straight = trunk.tails.values().next().value.points ? 0.999 : 0.95;
    for (let distance = length + spacing * 0.5; distance < item.spineLength - spacing * 0.3; distance += spacing) {
      const here = sample(distance), before = sample(distance - size * 1.5), after = sample(distance + size * 1.5);
      if (before.u.x * after.u.x + before.u.y * after.u.y < straight) continue;
      const u = here.u, m = { x: -u.y, y: u.x };
      chevrons.push(`M ${round(here.x - u.x * size * 0.7 + m.x * size)} ${round(here.y - u.y * size * 0.7 + m.y * size)} L ${round(here.x + u.x * size * 0.3)} ${round(here.y + u.y * size * 0.3)} L ${round(here.x - u.x * size * 0.7 - m.x * size)} ${round(here.y - u.y * size * 0.7 - m.y * size)}`);
    }
  }
  item.chevrons.replaceChildren(...chevrons.map((d) => svg("path", { d, class: "trunk-chevron" })));
  item.chevrons.style.strokeWidth = `${round(Math.max(1.4 * scale, strandWidth * 0.6))}px`;
  placeTag(item);
  if (item.lit) overlayTrunk(item, item.lit);
}
// The tag sits beside the arrowhead, on the side away from the ribbon's
// course, so it reads as the label of the trunk's target end. Its scale
// comes from CSS (--tag-scale), around the arrowhead's tip.
function placeTag(item) {
  if (!item.end) return;
  const { tip, dir, corners } = item.end;
  // Trunks enter a card's top or bottom; a curve may end at a slant.
  const vertical = Math.abs(dir.y) * 3 >= Math.abs(dir.x);
  item.tag.setAttribute("transform", vertical ? `translate(${round(Math.max(...corners.map((p) => p.x)) + 2)}, ${round(tip.y)})` : `translate(${round(tip.x)}, ${round(Math.min(...corners.map((p) => p.y)) - 2)})`);
  item.tag.classList.toggle("side", vertical);
  // Entering from above the plate ends at the tip (above the card); from
  // below it starts there. A sideways entry puts it above the arrowhead.
  const y = vertical ? (dir.y < 0 ? 0 : -22) : -22;
  for (const shape of item.shapes) {
    const x = vertical ? 0 : -shape.width / 2;
    shape.plate.setAttribute("x", round(x));
    shape.plate.setAttribute("y", y);
    shape.plate.setAttribute("width", round(shape.width));
    shape.strip.forEach((bar, i) => { bar.setAttribute("x", round(x + 6 + i * 5)); bar.setAttribute("y", y + 4); });
    shape.text.setAttribute("x", round(x + shape.width / 2 + shape.strip.length * 2.5));
    shape.text.setAttribute("y", y + 15.5);
  }
}
// Copies of the lit wires' strands over a dimmed trunk.
function overlayTrunk(item, edges) {
  item.lit = edges;
  const copies = [];
  item.strandPaths.forEach(({ edge, element }, i) => {
    if (!edges || !edges.has(edge) || !item.paths) return;
    const copy = svg("path", { d: item.paths[i], class: element.getAttribute("class") });
    copy.style.strokeWidth = element.style.strokeWidth;
    copies.push(copy);
  });
  item.overlay.replaceChildren(...copies);
}
// Trunks are laid out again at each step of the zoom (10%).
const zoomStep = (k) => Math.pow(1.1, Math.floor(Math.log(k) / Math.log(1.1) + 1e-9));
function updateWires() {
  const k = zoomStep(camera.k), graph = $("graph");
  if (scene.wireKey === k) return;
  scene.wireKey = k;
  graph.style.setProperty("--ts", String(Math.round(Math.min(12, Math.max(1, STRAND_MIN_PX / (Board.STRAND * k))) * 100) / 100));
  for (const item of scene.trunkEls || []) layoutTrunk(item, k);
}
function trunkScope(trunk) {
  const lit = { nodes: new Set([trunk.target]), edges: new Set(trunk.members) };
  for (const edge of trunk.members) lit.nodes.add(edge.from);
  return lit;
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
  // The title is fitted for the zoom (updateText); the other lines once.
  scene.cards.push({ element: titleText, full: title, box, width: box.width - corner, y: facts === null ? 37 : 30,
    compactWidth: box.width - 28 - (node.entry_kind === "group" ? 30 : 0) });
  group.fits = [[subtitleText, subtitle, box.width - 26, node.entry_kind === "file" || node.entry_kind === "group"]];
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
  if (!currentProjection) return;
  if (view === "dsm") { scene = buildScene(currentProjection); drawDsm(); updateFilterUI(); return; }
  if (view !== "diagram") return;
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
  updateText();
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
  $("view-dsm").setAttribute("aria-pressed", String(view === "dsm"));
  $("canvas").classList.toggle("table-mode", view === "table");
  $("canvas").classList.toggle("dsm-mode", view === "dsm");
  $("table-wrap").hidden = view !== "table";
  $("dsm-wrap").hidden = view !== "dsm";
  if (view !== "dsm") $("dsm-wrap").replaceChildren();
  if (view === "dsm") {
    $("table-wrap").replaceChildren();
    $("graph").replaceChildren();
    $("minimap").replaceChildren();
    $("legend").hidden = true;
    scene = buildScene(currentProjection);
    drawDsm();
  } else if (view === "diagram") {
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
// The dependency matrix of the level (dsm.js): the canvas's entries and
// wires with the same filters and directory groups, rows and columns in the
// canvas's order (bands, then layers from upper to lower), by name within
// each. Hovering a cell lights its row and column and the matching rows of
// the details panel; clicking it shows the dependency's evidence.
function dsmModel() {
  const inside = scene.entries.filter((node) => !node.outside_focus).map((node) => node.id);
  const outside = scene.entries.filter((node) => node.outside_focus).map((node) => node.id);
  const callers = outside.filter((id) => !scene.layoutEdges.some((edge) => edge.to === id));
  const others = outside.filter((id) => !callers.includes(id));
  const present = new Set(inside);
  let layers = (scene.layers && scene.layers.length ? scene.layers : clientLayers(inside, scene.layoutEdges)).map((ids) => ids.filter((id) => present.has(id)));
  const layered = new Set(layers.flat());
  const rest = inside.filter((id) => !layered.has(id));
  if (rest.length) layers = [...layers, rest];
  layers = layers.filter((ids) => ids.length);
  const name = (id) => displayEndpoint(id);
  const byName = (ids) => [...ids].sort((a, b) => name(a).localeCompare(name(b)) || a.localeCompare(b));
  const title = currentProjection.focus.title;
  const groups = [
    { label: "Outside this focus, depending on it", ids: byName(callers) },
    ...layers.map((ids, i) => ({ label: layers.length > 1 ? `${title}, layer ${i + 1} of ${layers.length}` : title, ids: byName(ids) })),
    { label: "Outside this focus", ids: byName(others) },
  ];
  const entriesInfo = new Map(scene.entries.map((node) => {
    const detail = displayEndpoint(node.id);
    const sub = node.entry_kind === "file" && node.file_path ? splitPath(node.file_path)[0] : null;
    return [node.id, { title: entryTitle(node), sub, detail: `${detail}${node.outside_focus ? " (outside this focus)" : ""}`, idle: isIdle(node) }];
  }));
  const cells = scene.edges.map((edge) => ({ from: edge.from, to: edge.to, count: edge.count, kinds: kindCounts(edge.parts), rules: edge.violation_rule_ids, edge }));
  const classes = [filters.violationsOnly ? "violations-only" : "", filters.idleOnly ? "idle-only" : ""].filter(Boolean).join(" ");
  return { groups, entries: entriesInfo, cells, classes };
}
function drawDsm() {
  scene.dsm = Dsm.render($("dsm-wrap"), dsmModel(), {
    cell: (edge, element) => showEdge(edge, element),
    entry: (id, element) => { const node = scene.byId.get(id); if (node) showNode(node, element); },
    open: (id) => {
      const node = scene.byId.get(id);
      if (node && node.entry_kind === "architecture") loadFocus(node.architecture_id);
      else if (node && node.entry_kind === "group") setGroupOpen(node, true);
    },
    hover: (source) => linkPanel(source),
  });
  // The selection survives a redraw (filters, groups).
  if (selected && selected.kind === "edge") {
    const edge = scene.edges.find((other) => other.from === selected.from && other.to === selected.to && other.origin === selected.origin);
    const cell = edge && scene.dsm.cellFor(edge);
    if (cell) cell.classList.add("selected");
  } else if (selected && selected.kind === "node") {
    const row = scene.dsm.rowFor(selected.id);
    if (row) row.classList.add("selected");
  }
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
  if (view === "diagram" || view === "dsm") redraw(); else updateFilterUI();
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
  const before = new Map([...(scene.positions || [])].map(([id, box]) => [id, { ...box }]));
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
  if (scene.trunks && scene.trunks.length) {
    const sample = svg("svg", { viewBox: "0 0 30 12", width: 30, height: 12, "aria-hidden": "true", class: "swatch edge trunk neutral" });
    sample.append(svg("path", { d: "M 2 1 Q 8 4.2 12 4.2", class: "edge-line" }), svg("path", { d: "M 2 11 Q 8 7.8 12 7.8", class: "edge-line" }));
    [4.2, 6, 7.8].forEach((y, i) => sample.append(svg("path", { d: `M 12 ${y} H 23`, class: `trunk-strand n${i}` })));
    sample.append(svg("path", { d: "M 29 6 L 23 2.5 L 23 9.5 Z", class: "trunk-arrow" }));
    const item = html("span", null, "legend-mark");
    item.title = "Wires from several entries into one target, merged into a ribbon with a strand per colour; the arrowhead marks the target, the tag gives their number";
    item.append(sample, html("span", "trunk"));
    marks.append(item);
  }
}
function updateDisplayUI() {
  for (const mode of MODES) $(`mode-${mode}`).setAttribute("aria-pressed", String(display.mode === mode));
  $("colour-by").value = display.colour;
  $("focus-mode").setAttribute("aria-pressed", String(display.focus));
  const dense = scene && scene.edges && scene.edges.length > FOCUS_EDGE_LIMIT;
  $("focus-mode").title = display.focus
    ? `Focus is on: ${dense ? "this level has" : "on levels with"} more than ${FOCUS_EDGE_LIMIT} wires, they are faint until you point at an entry, a wire or a legend row`
    : "Focus is off: every wire is drawn at full strength";
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
$("focus-mode").addEventListener("click", () => setDisplay({ focus: !display.focus }));
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
$("view-dsm").addEventListener("click", () => setView("dsm"));
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
