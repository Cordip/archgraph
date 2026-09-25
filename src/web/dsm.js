"use strict";

// The dependency structure matrix (DSM) view: one row and one column per
// entry of the level, in the same order on both axes, each cell the number
// of observed dependencies of the row's entry on the column's. The order
// follows the canvas: bands and layers from upper to lower, so a cell below
// the diagonal is a dependency against the layer order.
//
// Only DOM built with createElement and textContent; positions and sizes go
// through element.style (CSSOM), which the server's CSP allows, never a
// style attribute. Only the non-empty cells are elements, so a level of a
// thousand entries stays light; the matrix scrolls inside its own pane with
// sticky row and column headers.
const Dsm = (() => {
  const CELL = 24, ROW_HEAD = 280, COL_HEAD = 168;

  function el(tag, className, text) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    if (text !== undefined && text !== null) element.textContent = String(text);
    return element;
  }
  const px = (value) => `${value}px`;
  function countText(count) {
    if (count >= 10000) return `${Math.round(count / 1000)}k`;
    if (count >= 1000) return `${(count / 1000).toFixed(1)}k`;
    return String(count);
  }
  // Shading steps by the number of dependencies.
  const heat = (count) => (count >= 50 ? 4 : count >= 10 ? 3 : count >= 3 ? 2 : 1);

  // model: {
  //   groups: [{ label, ids }],   in order; each id appears once
  //   entries: Map id → { title, detail, idle },
  //   cells: [{ from, to, count, kinds: [[kind, count]], rules: [rule ids], edge }],
  //   classes: extra classes for the pane (filters)
  // }
  // hooks: { cell(edge, element), entry(id, element), open(id), hover(source) }
  function render(container, model, hooks) {
    const slots = []; // { group } or { id, number }
    const index = new Map();
    let number = 0;
    for (const group of model.groups) {
      if (!group.ids.length) continue;
      slots.push({ group });
      for (const id of group.ids) { index.set(id, slots.length); slots.push({ id, number: ++number }); }
    }
    const n = slots.length, size = n * CELL;
    const scroller = el("div", `dsm${model.classes ? ` ${model.classes}` : ""}`);
    scroller.setAttribute("role", "region");
    scroller.setAttribute("aria-label", `Dependency matrix: ${number} entries, ${model.cells.length} dependencies. Rows depend on columns.`);
    const grid = el("div", "dsm-grid");
    grid.style.gridTemplateColumns = `${px(ROW_HEAD)} ${px(size)}`;
    grid.style.gridTemplateRows = `${px(COL_HEAD)} ${px(size)}`;
    const corner = el("div", "dsm-corner");
    corner.append(el("p", "dsm-axis", "Rows depend on columns"), el("p", "dsm-key", `${number} entries, ${model.cells.length} dependencies`));
    const key = el("p", "dsm-key");
    key.append(el("span", "dsm-swatch violating"), document.createTextNode(" violation  "), el("span", "dsm-swatch upward"), document.createTextNode(" against the layers"));
    corner.append(key);
    const cols = el("div", "dsm-cols"), rows = el("div", "dsm-rows"), body = el("div", "dsm-body");
    cols.style.width = px(size);
    rows.style.height = px(size);
    body.style.width = px(size);
    body.style.height = px(size);
    const rowLabels = new Map(), colLabels = new Map();
    slots.forEach((slot, i) => {
      if (slot.group) {
        const rowHead = el("div", "dsm-group-label", slot.group.label);
        rowHead.style.top = px(i * CELL);
        rowHead.title = slot.group.label;
        const colHead = el("div", "dsm-col-label dsm-group-col", slot.group.label);
        colHead.style.left = px(i * CELL);
        colHead.title = slot.group.label;
        const bandRow = el("div", "dsm-band");
        bandRow.style.top = px(i * CELL);
        bandRow.style.width = px(size);
        const bandCol = el("div", "dsm-band");
        bandCol.style.left = px(i * CELL);
        bandCol.style.height = px(size);
        rows.append(rowHead);
        cols.append(colHead);
        body.append(bandRow, bandCol);
        return;
      }
      const entry = model.entries.get(slot.id);
      const row = el("button", `dsm-row-label${entry.idle ? " idle" : ""}`);
      row.type = "button";
      row.tabIndex = -1;
      row.style.top = px(i * CELL);
      row.title = `${slot.number}. ${entry.detail}`;
      row.append(el("span", "dsm-number", slot.number), el("span", "dsm-name", entry.title));
      if (entry.sub) row.append(el("span", "dsm-sub", entry.sub));
      row.dataset.id = slot.id;
      row.addEventListener("click", () => hooks.entry(slot.id, row));
      row.addEventListener("dblclick", () => hooks.open(slot.id));
      row.addEventListener("mouseenter", () => { light(i, null); hooks.hover({ node: slot.id }); });
      row.addEventListener("mouseleave", clear);
      rows.append(row);
      rowLabels.set(i, row);
      const col = el("div", "dsm-col-label");
      col.style.left = px(i * CELL);
      col.title = `${slot.number}. ${entry.detail}`;
      col.append(el("span", "dsm-number", slot.number), document.createTextNode(` ${entry.title}`));
      cols.append(col);
      colLabels.set(i, col);
      // The diagonal: an entry and itself, hatched like a section.
      const diagonal = el("div", "dsm-diag");
      diagonal.style.left = px(i * CELL);
      diagonal.style.top = px(i * CELL);
      body.append(diagonal);
    });
    // Highlight bars for the row and column under the pointer.
    const barRow = el("div", "dsm-hl dsm-hl-row"), barCol = el("div", "dsm-hl dsm-hl-col");
    barRow.style.width = px(size);
    barCol.style.height = px(size);
    barRow.hidden = barCol.hidden = true;
    body.append(barRow, barCol);
    let hot = [];
    function light(r, c) {
      for (const label of hot) label.classList.remove("hot");
      hot = [];
      barRow.hidden = r === null;
      barCol.hidden = c === null;
      if (r !== null) { barRow.style.top = px(r * CELL); if (rowLabels.has(r)) hot.push(rowLabels.get(r)); if (colLabels.has(r) && c === null) hot.push(colLabels.get(r)); }
      if (c !== null) { barCol.style.left = px(c * CELL); if (colLabels.has(c)) hot.push(colLabels.get(c)); }
      for (const label of hot) label.classList.add("hot");
    }
    function clear() { light(null, null); hooks.hover(null); }
    const cells = [], byEdge = new Map();
    const sorted = model.cells.filter((cell) => index.has(cell.from) && index.has(cell.to))
      .sort((a, b) => index.get(a.from) - index.get(b.from) || index.get(a.to) - index.get(b.to));
    for (const cell of sorted) {
      const r = index.get(cell.from), c = index.get(cell.to);
      const button = el("button", `dsm-cell h${heat(cell.count)}${cell.rules.length ? " violating" : ""}${r > c ? " upward" : ""}`, countText(cell.count));
      button.type = "button";
      button.tabIndex = cells.length ? -1 : 0;
      button.style.left = px(c * CELL);
      button.style.top = px(r * CELL);
      const from = model.entries.get(cell.from), to = model.entries.get(cell.to);
      button.title = `${from.detail}\n→ ${to.detail}\n${cell.kinds.map(([kind, count]) => `${kind} × ${count}`).join(", ")}${cell.rules.length ? `\n⚠ ${cell.rules.join(", ")}` : ""}${r > c ? "\nAgainst the layer order" : ""}`;
      button.setAttribute("aria-label", `${from.title} depends on ${to.title}: ${cell.kinds.map(([kind, count]) => `${kind} × ${count}`).join(", ")}${cell.rules.length ? `, violates ${cell.rules.join(", ")}` : ""}`);
      Object.assign(button.dataset, { from: cell.from, to: cell.to, row: r, col: c });
      button.addEventListener("click", () => hooks.cell(cell.edge, button));
      button.addEventListener("mouseenter", () => { light(r, c); hooks.hover({ edge: cell.edge }); });
      button.addEventListener("focus", () => { light(r, c); hooks.hover({ edge: cell.edge }); });
      button.addEventListener("blur", clear);
      button.addEventListener("keydown", (event) => move(event, button));
      body.append(button);
      cells.push(button);
      byEdge.set(cell.edge, button);
    }
    body.addEventListener("mouseleave", clear);
    // Arrow keys move between the non-empty cells: along the row or the
    // column when there is one that way, otherwise to the nearest.
    function move(event, from) {
      const steps = { ArrowRight: [0, 1], ArrowLeft: [0, -1], ArrowDown: [1, 0], ArrowUp: [-1, 0] };
      if (event.key === "Enter" || event.key === " ") return;
      const step = steps[event.key];
      if (!step) return;
      event.preventDefault();
      const r0 = Number(from.dataset.row), c0 = Number(from.dataset.col);
      let best = null, bestScore = Infinity;
      for (const cell of cells) {
        const dr = Number(cell.dataset.row) - r0, dc = Number(cell.dataset.col) - c0;
        const along = dr * step[0] + dc * step[1];
        if (along <= 0) continue;
        const score = along + 4 * Math.abs(dr * step[1] + dc * step[0]);
        if (score < bestScore) { bestScore = score; best = cell; }
      }
      if (!best) return;
      from.tabIndex = -1;
      best.tabIndex = 0;
      best.focus();
    }
    if (!cells.length) {
      const empty = el("p", "dsm-empty", number ? "No dependencies between these entries with the current filters." : "No entries at this level.");
      body.append(empty);
    }
    grid.append(corner, cols, rows, body);
    scroller.append(grid);
    container.replaceChildren(scroller);
    return { cellFor: (edge) => byEdge.get(edge) || null, rowFor: (id) => rows.querySelector(`[data-id="${CSS.escape(id)}"]`) };
  }
  return { render, CELL };
})();
