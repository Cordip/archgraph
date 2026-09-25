"use strict";

// Board routing for the PCB and Hex display modes. Pure geometry: no DOM, no
// storage, and every sort has an explicit tie-break, so the same input always
// gives the same routing.
//
// Cards sit on a grid of rows (the layers of the layout) and slots. Traces run
// in channels: the horizontal gaps between rows, plus
//   - PCB: vertical channels between columns; corners are chamfered at 45°;
//   - Hex: 60° strips through the gaps between hexagonal cards, whose rows are
//     offset by half a slot like the cells of a honeycomb.
// Each channel gives every trace its own track at a constant pitch, ordered to
// reduce crossings, and grows when it needs more tracks. Where two traces
// still cross, one hops over the other with a small bridge.
//
// Traces marked with the same trunk key and heading the same way (at least
// `trunkMin` of them) form a trunk, like a cable harness: they share the
// target's port, one vertical channel (PCB) or one strip per row (Hex), and
// one track in every channel they pass, so their courses coincide from the
// point where each joins. trunkRuns then finds how many traces run along
// each piece, for drawing the trunk thicker where more of them share it.
const Board = (() => {
  const PITCH = 12; // distance between neighbouring tracks
  const CHAMFER = 8; // PCB corner cut
  const H_MARGIN = 20; // between a card edge and the nearest track
  const V_MARGIN = 16;
  const MIN_GAP_X = 40, MIN_GAP_Y = 56, BAND_GAP = 124;
  const PORT_INSET = 18, PORT_GAP = 22, HEX_PORT_GAP = 24;
  const HEX_MIN_GAP = 40, HEX_GAP_MARGIN = 14;
  // Past this many traces through one gap, the uniform honeycomb would grow
  // too wide to read (and to route quickly).
  const HEX_MAX_STRIP = 40;
  const HOP = { height: 4, flat: 6 };
  // Trunk widths by how many traces share a piece; the widest stays well
  // inside the track pitch.
  const TRUNK_TIERS = [{ min: 2, width: 3 }, { min: 4, width: 4.5 }, { min: 8, width: 6 }, { min: 16, width: 7.5 }];
  const ORIGIN = { x: 64, y: 40 };
  const SQRT3 = Math.sqrt(3), SIN60 = SQRT3 / 2;
  const EPS = 1e-6;

  const round = (value) => Math.round(value * 10) / 10;
  const sign = (value) => (value > EPS ? 1 : value < -EPS ? -1 : 0);
  const distance = (p, q) => Math.hypot(q.x - p.x, q.y - p.y);
  const toward = (from, to, length) => {
    const d = distance(from, to) || 1;
    return { x: from.x + (to.x - from.x) * length / d, y: from.y + (to.y - from.y) * length / d };
  };
  // Lexicographic comparison of score arrays.
  const less = (a, b) => { for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return a[i] < b[i]; return false; };

  // ------------------------------------------------------------ channels
  // A leg is one trace's run along a channel: its span [lo, hi] on the
  // channel's axis and its two stubs, which leave the track towards one side
  // (-1: towards track 0, +1: away from it). Putting P on a lower track than Q
  // costs a crossing for each P stub towards Q that lands inside Q's span, and
  // for each Q stub towards P inside P's.
  function crossings(p, q) {
    let cost = 0;
    for (const stub of p.stubs) if (stub.side > 0 && stub.at > q.lo + EPS && stub.at < q.hi - EPS) cost++;
    for (const stub of q.stubs) if (stub.side < 0 && stub.at > p.lo + EPS && stub.at < p.hi - EPS) cost++;
    return cost;
  }
  // Track order with few crossings: each leg is inserted where it adds the
  // fewest, then every leg is reinserted (twice, or less in a very busy
  // channel, where each pass costs the square of its legs). Ties keep legs
  // in the order of their spans.
  function orderLegs(legs) {
    const sorted = [...legs].sort((a, b) => (a.lo + a.hi) - (b.lo + b.hi) || a.key - b.key || a.part - b.part);
    sorted.forEach((leg, index) => { leg.rank = index; });
    const list = [];
    const before = [], after = [];
    const insert = (leg) => {
      let cost = 0, natural = 0;
      for (let k = 0; k < list.length; k++) {
        const other = list[k];
        before[k] = crossings(other, leg);
        after[k] = crossings(leg, other);
        cost += after[k];
        if (other.rank < leg.rank) natural++;
      }
      let best = 0, bestCost = cost, bestShift = natural;
      for (let k = 0; k < list.length; k++) {
        cost += before[k] - after[k];
        const shift = Math.abs(k + 1 - natural);
        if (cost < bestCost || (cost === bestCost && shift < bestShift)) { best = k + 1; bestCost = cost; bestShift = shift; }
      }
      list.splice(best, 0, leg);
    };
    for (const leg of sorted) insert(leg);
    const passes = sorted.length <= 150 ? 2 : sorted.length <= 500 ? 1 : 0;
    for (let pass = 0; pass < passes; pass++) {
      for (const leg of sorted) { list.splice(list.indexOf(leg), 1); insert(leg); }
    }
    return list;
  }
  // Tracks in the chosen order. With `share`, legs whose spans are at least
  // `gap` apart may use the same track (a channel router's left-edge rule);
  // overlapping legs never do.
  function packLegs(list, share, gap) {
    const track = new Map();
    let count = 0;
    list.forEach((leg, index) => {
      let t = 0;
      if (share) {
        for (let j = 0; j < index; j++) {
          const other = list[j];
          if (other.lo < leg.hi + gap && leg.lo < other.hi + gap) t = Math.max(t, track.get(other) + 1);
        }
      } else t = index;
      track.set(leg, t);
      count = Math.max(count, t + 1);
    });
    return { track, count };
  }
  function assign(groups, share, gap) {
    const track = new Map(), count = new Map();
    for (const [key, legs] of groups) {
      const packed = packLegs(orderLegs(legs), share, gap);
      for (const [leg, t] of packed.track) track.set(leg, t);
      count.set(key, packed.count);
    }
    return { track, count };
  }
  function push(map, key, value) {
    if (!map.has(key)) map.set(key, []);
    map.get(key).push(value);
  }
  // Where track t of n sits in a channel [start, start + size]: the tracks
  // are centred in the channel.
  const trackAt = (start, size, n, t) => start + (size - n * PITCH) / 2 + (t + 0.5) * PITCH;

  // ------------------------------------------------------------ grid
  // Strictly increasing slots in [0, columns) closest to the wanted ones
  // (dynamic programming over at most a few dozen cells).
  function monotone(want, columns) {
    const n = want.length;
    if (!n) return [];
    const cost = [], from = [];
    for (let i = 0; i < n; i++) {
      cost.push(new Array(columns).fill(Infinity));
      from.push(new Array(columns).fill(-1));
      for (let c = i; c <= columns - n + i; c++) {
        const own = Math.abs(c - want[i]);
        if (i === 0) { cost[0][c] = own; continue; }
        for (let p = i - 1; p < c; p++) {
          if (cost[i - 1][p] + own < cost[i][c] - EPS) { cost[i][c] = cost[i - 1][p] + own; from[i][c] = p; }
        }
      }
    }
    let best = n - 1;
    for (let c = n - 1; c < columns; c++) if (cost[n - 1][c] < cost[n - 1][best] - EPS) best = c;
    const result = new Array(n);
    for (let i = n - 1; i >= 0; i--) { result[i] = best; best = from[i][best]; }
    return result;
  }
  // Rows keep the layout's order; each card takes the slot nearest the
  // centre the layout wanted for it. Cards the user dragged (`fixed`, id →
  // [row, slot]) keep their slot, and the cards they displace take the
  // nearest free one.
  function placeGrid(input, hex) {
    const { rows, centre, unit } = input;
    const columns = Math.max(1, ...rows.map((row) => row.ids.length));
    const cell = new Map();
    rows.forEach((row, r) => {
      const shift = hex && r % 2 ? 0.5 : 0;
      const slots = monotone(row.ids.map((id) => (centre.get(id) || 0) / unit - 0.5 - shift), columns);
      row.ids.forEach((id, i) => cell.set(id, { row: r, col: slots[i] }));
    });
    const key = (r, c) => `${r}:${c}`;
    const final = new Map(), used = new Set();
    let maxRow = rows.length - 1, maxCol = columns - 1;
    for (const id of [...(input.fixed || new Map()).keys()].filter((id) => cell.has(id)).sort()) {
      const [r, c] = input.fixed.get(id);
      if (!Number.isInteger(r) || !Number.isInteger(c) || r < 0 || c < 0 || r > rows.length + 4 || c > columns + 4 || used.has(key(r, c))) continue;
      final.set(id, { row: r, col: c });
      used.add(key(r, c));
      maxRow = Math.max(maxRow, r);
      maxCol = Math.max(maxCol, c);
    }
    const displaced = [];
    for (const [id, place] of cell) {
      if (final.has(id)) continue;
      if (used.has(key(place.row, place.col))) displaced.push(id);
      else { final.set(id, place); used.add(key(place.row, place.col)); }
    }
    for (const id of displaced) {
      const want = cell.get(id);
      let best = null;
      for (let r = 0; r <= maxRow + 1; r++) {
        for (let c = 0; c <= maxCol + 1; c++) {
          if (used.has(key(r, c))) continue;
          const score = [Math.abs(r - want.row) * 3 + Math.abs(c - want.col), r, c];
          if (!best || less(score, best)) best = score;
        }
      }
      final.set(id, { row: best[1], col: best[2] });
      used.add(key(best[1], best[2]));
      maxRow = Math.max(maxRow, best[1]);
      maxCol = Math.max(maxCol, best[2]);
    }
    let R = 0, C = 0;
    for (const place of final.values()) { R = Math.max(R, place.row + 1); C = Math.max(C, place.col + 1); }
    const bands = [];
    for (let r = 0; r < R; r++) bands.push(rows[Math.min(r, rows.length - 1)] ? rows[Math.min(r, rows.length - 1)].band : "inside");
    return { cell: final, R, C, bands };
  }

  // The route of each edge between rows: its direction, the channel it
  // leaves its source into and the channel it reaches its target from.
  function trips(edges, cell) {
    return edges.map((edge, index) => {
      const a = cell.get(edge.from), b = cell.get(edge.to);
      const dir = Math.sign(b.row - a.row);
      return { index, edge, a, b, dir,
        first: dir > 0 ? a.row : a.row - 1,
        last: dir > 0 ? b.row - 1 : dir < 0 ? b.row : a.row - 1,
        srcSide: dir > 0 ? "bottom" : "top", dstSide: dir < 0 ? "bottom" : "top" };
    });
  }
  // Trunks: traces with the same key heading the same way (down or along a
  // row, or up), when there are at least `min` of them.
  function markTrunks(list, min) {
    const groups = new Map();
    for (const trip of list) if (trip.edge.trunk) push(groups, `${trip.edge.trunk}\u0000${trip.dir < 0 ? "up" : "down"}`, trip);
    for (const [id, members] of groups) if (members.length >= min) for (const trip of members) trip.trunk = id;
  }
  function addStubs(leg, stubs) {
    for (const stub of stubs) if (!leg.stubs.some((other) => Math.abs(other.at - stub.at) < EPS && other.side === stub.side)) leg.stubs.push(stub);
  }
  // A trunk's traces take one track in each channel: a shared leg spanning
  // all of theirs stands in for them when tracks are ordered and packed.
  function channelLegs(list) {
    const groups = new Map(), shared = new Map();
    for (const trip of list) {
      for (const item of trip.legs) {
        if (!item) continue;
        if (!trip.trunk) { push(groups, item.h, item); continue; }
        const key = `${trip.trunk}\u0000${item.h}`;
        let proxy = shared.get(key);
        if (!proxy) {
          proxy = { trip, key: trip.index, part: item.part, h: item.h, lo: item.lo, hi: item.hi, stubs: [] };
          shared.set(key, proxy);
          push(groups, item.h, proxy);
        }
        proxy.lo = Math.min(proxy.lo, item.lo);
        proxy.hi = Math.max(proxy.hi, item.hi);
        addStubs(proxy, item.stubs);
        item.proxy = proxy;
      }
    }
    return groups;
  }
  // Ports spread along a card side, ordered by where their traces head so
  // that neighbouring traces leave the card without crossing. A trunk's
  // traces share one port at their target.
  function spreadPorts(tripList, position, towardFor, gap) {
    const sides = new Map();
    for (const trip of tripList) {
      push(sides, `${trip.edge.from}\u0000${trip.srcSide}`, { trip, end: 0, toward: towardFor(trip, 0) });
      push(sides, `${trip.edge.to}\u0000${trip.dstSide}`, { trip, end: 1, toward: towardFor(trip, 1) });
    }
    for (const [key, items] of sides) {
      const box = position(key.split("\u0000")[0]);
      const slots = [], byTrunk = new Map();
      for (const item of items) {
        const shared = item.end === 1 && item.trip.trunk;
        let slot = shared ? byTrunk.get(item.trip.trunk) : null;
        if (!slot) {
          slot = { items: [], toward: 0, index: item.trip.index, end: item.end };
          slots.push(slot);
          if (shared) byTrunk.set(item.trip.trunk, slot);
        }
        slot.items.push(item);
      }
      for (const slot of slots) slot.toward = slot.items.reduce((sum, item) => sum + item.toward, 0) / slot.items.length;
      slots.sort((p, q) => p.toward - q.toward || p.index - q.index || p.end - q.end);
      const n = slots.length;
      const spacing = n > 1 ? Math.min(gap, (box.width - 2 * PORT_INSET) / (n - 1)) : 0;
      slots.forEach((slot, i) => {
        const x = box.x + box.width / 2 + (i - (n - 1) / 2) * spacing;
        for (const item of slot.items) { if (item.end) item.trip.dstX = x; else item.trip.srcX = x; }
      });
    }
  }
  function sideCounts(tripList) {
    const counts = new Map(), seen = new Set();
    const add = (key, trunk) => {
      if (trunk) { if (seen.has(`${key}\u0000${trunk}`)) return; seen.add(`${key}\u0000${trunk}`); }
      counts.set(key, (counts.get(key) || 0) + 1);
    };
    for (const trip of tripList) { add(`${trip.edge.from}\u0000${trip.srcSide}`, null); add(`${trip.edge.to}\u0000${trip.dstSide}`, trip.trunk); }
    return (id) => Math.max(counts.get(`${id}\u0000top`) || 0, counts.get(`${id}\u0000bottom`) || 0);
  }
  // Heights of the horizontal channels (index 0 is above the first row) and
  // the top of each row. Channels between bands leave room for the focus
  // frame and the band's note.
  function rowTops(hCount, grid, card) {
    const { R, bands } = grid;
    const heights = [];
    for (let h = -1; h < R; h++) {
      const n = hCount.get(h) || 0, outer = h < 0 || h === R - 1;
      let height = n ? n * PITCH + 2 * H_MARGIN : 0;
      if (!outer) height = Math.max(height, MIN_GAP_Y);
      if (!outer && bands[h] !== bands[h + 1]) height = Math.max(height, BAND_GAP);
      if (h < 0 && bands[0] !== "inside") height = Math.max(height, 40);
      heights.push(height);
    }
    const rowY = [];
    let y = ORIGIN.y + heights[0];
    for (let r = 0; r < R; r++) { rowY.push(y); y += card.height + heights[r + 1]; }
    const start = (h) => (h < 0 ? rowY[0] - heights[0] : rowY[h] + card.height);
    const trackY = (h, t) => trackAt(start(h), heights[h + 1], hCount.get(h) || 0, t);
    const notes = [];
    for (let r = 0; r < R; r++) if (bands[r] !== "inside" && (r === 0 || bands[r - 1] !== bands[r])) notes.push({ y: rowY[r] - 18, band: bands[r] });
    return { rowY, heights, trackY, notes };
  }
  // Removes repeated points and points in the middle of a straight run.
  function simplify(points) {
    const out = [];
    for (const point of points) {
      if (out.length && distance(out[out.length - 1], point) < 0.5) continue;
      if (out.length >= 2) {
        const p = out[out.length - 2], q = out[out.length - 1];
        if (Math.abs((q.x - p.x) * (point.y - q.y) - (q.y - p.y) * (point.x - q.x)) < 0.5 * distance(p, q)
          && (q.x - p.x) * (point.x - q.x) + (q.y - p.y) * (point.y - q.y) > 0) out.pop();
      }
      out.push(point);
    }
    return out;
  }
  function chamfer(points, size) {
    const out = [points[0]];
    for (let i = 1; i + 1 < points.length; i++) {
      const p = points[i - 1], q = points[i], r = points[i + 1];
      const c = Math.min(size, distance(p, q) / 2, distance(q, r) / 2);
      if (c < 0.5) out.push(q);
      else out.push(toward(q, p, c), toward(q, r, c));
    }
    out.push(points[points.length - 1]);
    return out;
  }
  // The arrow tip stops just short of the card.
  function shorten(points, length) {
    const n = points.length;
    points[n - 1] = toward(points[n - 1], points[n - 2], length);
    return points;
  }

  // ------------------------------------------------------------ PCB
  function routePcb(input) {
    const grid = placeGrid(input, false);
    const { cell, R, C } = grid;
    const H = input.card.height;
    const list = trips(input.edges, cell);
    markTrunks(list, input.trunkMin || 3);
    // Vertical channel j lies right of column j (-1: left of the first). A
    // trace that skips rows takes the channel closest to its straight course;
    // among equals, the least used one. A trunk takes one channel for all
    // its traces, closest to their courses together.
    const load = new Array(C + 1).fill(0);
    const longest = list.filter((trip) => trip.first !== trip.last)
      .sort((p, q) => Math.abs(q.last - q.first) - Math.abs(p.last - p.first) || p.index - q.index);
    const riders = new Map(), spine = new Map();
    for (const trip of longest) if (trip.trunk) push(riders, trip.trunk, trip);
    for (const trip of longest) {
      if (trip.trunk && spine.has(trip.trunk)) { trip.v = spine.get(trip.trunk); continue; }
      const group = trip.trunk ? riders.get(trip.trunk) : [trip];
      const xb = trip.b.col + 0.5;
      let best = null;
      for (let j = -1; j < C; j++) {
        const u = j + 1;
        let course = 0, middle = 0;
        for (const other of group) {
          const xa = other.a.col + 0.5;
          course += Math.abs(xa - u) + Math.abs(u - xb);
          middle += Math.abs(u - (xa + xb) / 2);
        }
        const score = [course, load[u], middle, j];
        if (!best || less(score, best.score)) best = { j, score };
      }
      trip.v = best.j;
      load[best.j + 1]++;
      if (trip.trunk) spine.set(trip.trunk, best.j);
    }
    const vGroups = new Map(), vShared = new Map();
    for (const trip of list) {
      if (trip.v === undefined) continue;
      const u = trip.v + 1;
      const own = { trip, key: trip.index, part: 0, lo: Math.min(trip.first, trip.last), hi: Math.max(trip.first, trip.last),
        stubs: [{ at: trip.first, side: trip.a.col + 0.5 < u ? -1 : 1 }, { at: trip.last, side: trip.b.col + 0.5 < u ? -1 : 1 }] };
      if (!trip.trunk) { trip.vLeg = own; push(vGroups, trip.v, own); continue; }
      let shared = vShared.get(trip.trunk);
      if (!shared) { shared = { ...own, stubs: [] }; vShared.set(trip.trunk, shared); push(vGroups, trip.v, shared); }
      shared.lo = Math.min(shared.lo, own.lo);
      shared.hi = Math.max(shared.hi, own.hi);
      addStubs(shared, own.stubs);
      trip.vLeg = shared;
    }
    const vertical = assign(vGroups, true, 0.5);
    // Columns widen when a card needs more ports than fit at the pitch.
    const ports = sideCounts(list);
    const colW = new Array(C).fill(input.card.width);
    for (const [id, place] of cell) colW[place.col] = Math.max(colW[place.col], (ports(id) - 1) * PITCH + 2 * PORT_INSET);
    const vW = [];
    for (let j = -1; j < C; j++) {
      const n = vertical.count.get(j) || 0, outer = j < 0 || j === C - 1;
      vW.push(Math.max(outer ? 0 : MIN_GAP_X, n ? n * PITCH + 2 * V_MARGIN : 0));
    }
    const colX = [];
    let x = ORIGIN.x + vW[0];
    for (let c = 0; c < C; c++) { colX.push(x); x += colW[c] + vW[c + 1]; }
    const vStart = (j) => (j < 0 ? colX[0] - vW[0] : colX[j] + colW[j]);
    for (const trip of list) {
      if (trip.v !== undefined) trip.vx = trackAt(vStart(trip.v), vW[trip.v + 1], vertical.count.get(trip.v), vertical.track.get(trip.vLeg));
    }
    const box = (id) => { const place = cell.get(id); return { x: colX[place.col], width: colW[place.col] }; };
    const centre = (place) => colX[place.col] + colW[place.col] / 2;
    spreadPorts(list, box, (trip, end) => (trip.vx !== undefined ? trip.vx : centre(end ? trip.a : trip.b)), PORT_GAP);
    // Horizontal channel h lies below row h (-1: above the first row). A
    // port on a card's bottom meets the channel below from its top side.
    const sideOf = (cardSide) => (cardSide === "bottom" ? -1 : 1);
    const leg = (trip, h, part, p, q) => {
      if (Math.abs(p.x - q.x) < 0.5) return null; // a straight drop needs no track
      return { trip, key: trip.index, part, h, lo: Math.min(p.x, q.x), hi: Math.max(p.x, q.x), stubs: [{ at: p.x, side: p.side }, { at: q.x, side: q.side }] };
    };
    for (const trip of list) {
      const src = { x: trip.srcX, side: sideOf(trip.srcSide) }, dst = { x: trip.dstX, side: sideOf(trip.dstSide) };
      if (trip.v === undefined) trip.legs = [leg(trip, trip.first, 0, src, dst)];
      else trip.legs = [leg(trip, trip.first, 0, src, { x: trip.vx, side: trip.dir > 0 ? 1 : -1 }), leg(trip, trip.last, 1, { x: trip.vx, side: trip.dir > 0 ? -1 : 1 }, dst)];
    }
    const horizontal = assign(channelLegs(list), true, 2 * PITCH);
    const { rowY, trackY, notes } = rowTops(horizontal.count, grid, input.card);
    const positions = new Map();
    for (const [id, place] of cell) positions.set(id, { x: colX[place.col], y: rowY[place.row], width: colW[place.col], height: H, row: place.row, col: place.col });
    const traces = list.map((trip) => {
      const a = positions.get(trip.edge.from), b = positions.get(trip.edge.to);
      const start = { x: trip.srcX, y: trip.srcSide === "bottom" ? a.y + a.height : a.y };
      const end = { x: trip.dstX, y: trip.dstSide === "bottom" ? b.y + b.height : b.y };
      const points = [start];
      const legY = (item) => trackY(item.h, horizontal.track.get(item.proxy || item));
      if (trip.v === undefined) {
        if (trip.legs[0]) { const y = legY(trip.legs[0]); points.push({ x: start.x, y }, { x: end.x, y }); }
      } else {
        const y1 = legY(trip.legs[0]), y2 = legY(trip.legs[1]);
        points.push({ x: start.x, y: y1 }, { x: trip.vx, y: y1 }, { x: trip.vx, y: y2 }, { x: end.x, y: y2 });
      }
      points.push(end);
      return { dir: trip.dir === 0 ? "same" : trip.dir > 0 ? "down" : "up", trunk: trip.trunk || null, core: shorten(simplify(chamfer(simplify(points), CHAMFER)), 2) };
    });
    const colPitch = input.card.width + MIN_GAP_X;
    const grid2 = { mode: "pcb", R, C, rowY, colX, colW, height: H, rowPitch: H + MIN_GAP_Y, colPitch };
    return finish(input, positions, traces, notes, grid2, 45);
  }

  // ------------------------------------------------------------ Hex
  // A hexagonal card: the card's rectangle with pointed ends whose sides run
  // at 60°, so the gap between two neighbours holds straight 60° strips.
  const earOf = (height) => height / (2 * SQRT3);
  function routeHex(input) {
    const grid = placeGrid(input, true);
    const { cell, R, C } = grid;
    const H = input.card.height, ear = earOf(H), rise = H / SQRT3; // horizontal shift across a row
    const list = trips(input.edges, cell);
    markTrunks(list, input.trunkMin || 3);
    const ports = sideCounts(list);
    let W = input.card.width;
    for (const id of cell.keys()) W = Math.max(W, (ports(id) - 1) * HEX_PORT_GAP + 2 * PORT_INSET);
    // Rows crossed on the way: the gap in each and the direction through it,
    // chosen in slot units before the gap width is known.
    const estimate = W + 2 * ear + HEX_MIN_GAP;
    const slot = (r, c) => c + 0.5 + (r % 2) * 0.5;
    const load = new Map();
    const chooseStrips = (trip) => {
      trip.strips = [];
      let x = slot(trip.a.row, trip.a.col);
      const target = slot(trip.b.row, trip.b.col);
      const waypoints = [x];
      if (trip.dir) {
        for (let k = trip.a.row + trip.dir; k !== trip.b.row; k += trip.dir) {
          let best = null;
          for (let g = -1; g < C; g++) {
            const gx = g + 1 + (k % 2) * 0.5;
            // Every gap has the width of the busiest one, so traces spread
            // out: sixteen more traces are worth a detour of one slot.
            const used = load.get(`${k}:${g}`) || 0;
            const score = [Math.abs(x - gx) + Math.abs(gx - target) + used / 16, used, Math.abs(gx - (x + target) / 2), g];
            if (!best || less(score, best.score)) best = { g, gx, score };
          }
          const o = sign(target - best.gx) || sign(best.gx - x) || 1; // horizontal heading
          trip.strips.push({ row: k, gap: best.g, slope: trip.dir > 0 ? o : -o });
          load.set(`${k}:${best.g}`, (load.get(`${k}:${best.g}`) || 0) + 1);
          x = best.gx + o * rise / estimate / 2;
          waypoints.push(best.gx);
        }
      }
      waypoints.push(target);
      trip.strips.forEach((strip, i) => { strip.key = (waypoints[i] + waypoints[i + 2]) / 2; strip.owner = trip; });
    };
    // A trunk follows the strips of its longest trace; each of its traces
    // takes the strips of the rows it crosses.
    const riders = new Map(), spines = new Map();
    for (const trip of list) if (trip.trunk) push(riders, trip.trunk, trip);
    for (const trip of list) {
      if (!trip.trunk) { chooseStrips(trip); continue; }
      if (!spines.has(trip.trunk)) {
        const lead = riders.get(trip.trunk).reduce((best, other) => (Math.abs(other.b.row - other.a.row) > Math.abs(best.b.row - best.a.row) ? other : best));
        chooseStrips(lead);
        spines.set(trip.trunk, new Map(lead.strips.map((strip) => [strip.row, strip])));
      }
      const rows = spines.get(trip.trunk);
      trip.strips = [];
      if (trip.dir) for (let k = trip.a.row + trip.dir; k !== trip.b.row; k += trip.dir) trip.strips.push(rows.get(k));
    }
    // Tracks across each strip, ordered by where the traces come from and
    // go; the gap between cards fits the busiest strip. A second pass turns
    // each strip towards its real neighbours, known only once ports exist.
    let gap = HEX_MIN_GAP, P = estimate;
    const rectX = (r, c) => ORIGIN.x + ear + (c + (r % 2) * 0.5) * P;
    const gapMid = (r, g) => rectX(r, 0) - ear - gap / 2 + (g + 1) * P;
    const centreOf = (place) => rectX(place.row, place.col) + W / 2;
    const layoutStrips = () => {
      const strips = new Map(), seen = new Set();
      for (const trip of list) {
        for (const strip of trip.strips) {
          if (seen.has(strip)) continue;
          seen.add(strip);
          push(strips, `${strip.row}:${strip.gap}:${strip.slope}`, { trip: strip.owner, strip });
        }
      }
      let widest = 0;
      for (const key of [...strips.keys()].sort()) {
        const items = strips.get(key);
        items.sort((p, q) => p.strip.key - q.strip.key || p.trip.index - q.trip.index);
        items.forEach((item, i) => { item.strip.offset = (i - (items.length - 1) / 2) * PITCH / SIN60; });
        widest = Math.max(widest, items.length);
      }
      gap = Math.max(HEX_MIN_GAP, (widest - 1) * PITCH / SIN60 + 2 * HEX_GAP_MARGIN);
      P = W + 2 * ear + gap;
      for (const trip of list) for (const strip of trip.strips) strip.mid = gapMid(strip.row, strip.gap) + strip.offset;
      spreadPorts(list, (id) => { const place = cell.get(id); return { x: rectX(place.row, place.col), width: W }; }, (trip, end) => {
        if (!trip.strips.length) return centreOf(end ? trip.a : trip.b);
        return trip.strips[end ? trip.strips.length - 1 : 0].mid;
      }, HEX_PORT_GAP);
      return widest;
    };
    if (layoutStrips() > HEX_MAX_STRIP) {
      return { ...routePcb(input), fallback: `Too many wires cross the rows of this level for the hexagonal board (up to ${HEX_MAX_STRIP} per gap), so it is drawn as the PCB board.` };
    }
    for (const trip of list) {
      const xs = [trip.srcX, ...trip.strips.map((strip) => strip.mid), trip.dstX];
      trip.strips.forEach((strip, i) => {
        if (strip.owner !== trip) return; // a trunk's strip turns with its longest trace
        const o = sign(xs[i + 2] - strip.mid) || sign(strip.mid - xs[i]) || 1;
        strip.slope = trip.dir > 0 ? o : -o;
        strip.key = (xs[i] + xs[i + 2]) / 2;
      });
    }
    layoutStrips();
    // Legs along the horizontal channels. A strip end sits where the strip's
    // line meets the row's top or bottom edge; `dir` is the way its line
    // moves sideways as it enters the channel (null: a port, free to choose).
    for (const trip of list) {
      const ends = [{ x: trip.srcX, side: trip.srcSide === "bottom" ? -1 : 1, dir: null, port: true }];
      for (const strip of trip.strips) {
        const s = strip.slope;
        if (trip.dir > 0) ends.push({ x: strip.mid - s * rise / 2, side: 1, dir: -s, row: strip.row, edge: "top" }, { x: strip.mid + s * rise / 2, side: -1, dir: s, row: strip.row, edge: "bottom" });
        else ends.push({ x: strip.mid + s * rise / 2, side: -1, dir: s, row: strip.row, edge: "bottom" }, { x: strip.mid - s * rise / 2, side: 1, dir: -s, row: strip.row, edge: "top" });
      }
      ends.push({ x: trip.dstX, side: trip.dstSide === "bottom" ? -1 : 1, dir: null, port: true });
      trip.legs = [];
      for (let i = 0; i + 1 < ends.length; i += 2) {
        const part = i / 2;
        const h = trip.dir > 0 ? trip.a.row + part : trip.dir < 0 ? trip.a.row - 1 - part : trip.a.row - 1;
        const p = ends[i], q = ends[i + 1];
        const item = { trip, key: trip.index, part, h, p, q, lo: Math.min(p.x, q.x), hi: Math.max(p.x, q.x) + (Math.abs(p.x - q.x) < 0.5 ? 1 : 0),
          stubs: [{ at: p.x, side: p.side }, { at: q.x, side: q.side }] };
        trip.legs.push(item);
      }
    }
    const hGroups = channelLegs(list);
    // Legs share a track only when they stay apart even after their slanted
    // stubs shift them sideways, by at most the channel's height / √3.
    const horizontal = { track: new Map(), count: new Map() };
    for (const [h, legs] of hGroups) {
      const order = orderLegs(legs);
      const tallest = order.length * PITCH + 2 * H_MARGIN;
      const packed = packLegs(order, true, 2 * PITCH + 2 * tallest / SQRT3);
      for (const [item, t] of packed.track) horizontal.track.set(item, t);
      horizontal.count.set(h, packed.count);
    }
    const { rowY, trackY, notes } = rowTops(horizontal.count, grid, input.card);
    const positions = new Map();
    for (const [id, place] of cell) positions.set(id, { x: rectX(place.row, place.col), y: rowY[place.row], width: W, height: H, row: place.row, col: place.col, hex: ear });
    const traces = list.map((trip) => {
      const a = positions.get(trip.edge.from), b = positions.get(trip.edge.to);
      const edgeY = (end, item) => {
        if (end.port) return end === item.p ? (trip.srcSide === "bottom" ? a.y + H : a.y) : (trip.dstSide === "bottom" ? b.y + H : b.y);
        return end.edge === "top" ? rowY[end.row] : rowY[end.row] + H;
      };
      const points = [];
      for (const item of trip.legs) {
        const y = trackY(item.h, horizontal.track.get(item.proxy || item));
        const yp = edgeY(item.p, item), yq = edgeY(item.q, item);
        const dp = Math.abs(y - yp) / SQRT3, dq = Math.abs(y - yq) / SQRT3;
        // Free (port) ends turn towards the other end; the choice avoiding
        // sharp 120° hooks wins.
        const options = (end, other) => (end.dir !== null ? [end.dir] : [sign(other.x - end.x) || 1, -(sign(other.x - end.x) || 1)]);
        let best = null;
        for (const sp of options(item.p, item.q)) {
          for (const sq of options(item.q, item.p)) {
            const xp = item.p.x + sp * dp, xq = item.q.x + sq * dq, h = xq - xp;
            const sharp = (Math.abs(h) > 0.5 && sign(h) !== sp ? 1 : 0) + (Math.abs(h) > 0.5 && sign(h) !== -sq ? 1 : 0);
            const score = [sharp, Math.abs(h)];
            if (!best || less(score, best.score)) best = { score, xp, xq };
          }
        }
        if (item === trip.legs[0]) points.push({ x: item.p.x, y: yp });
        points.push({ x: best.xp, y }, { x: best.xq, y });
        if (item === trip.legs[trip.legs.length - 1]) points.push({ x: item.q.x, y: yq });
      }
      return { dir: trip.dir === 0 ? "same" : trip.dir > 0 ? "down" : "up", trunk: trip.trunk || null, core: shorten(simplify(points), 2) };
    });
    const grid2 = { mode: "hex", R, C, rowY, height: H, rowPitch: H + MIN_GAP_Y, P, W, ear, gap };
    return finish(input, positions, traces, notes, grid2, 60);
  }

  // ------------------------------------------------------------ bridges
  // Segments of all traces in a coarse bucket grid, for crossing and label
  // tests without comparing every pair.
  const BUCKET = 96;
  function segmentIndex(traces, field) {
    const segments = [], buckets = new Map();
    traces.forEach((trace, ti) => {
      const points = trace[field];
      for (let i = 0; i + 1 < points.length; i++) {
        const a = points[i], b = points[i + 1];
        const index = segments.push({ ti, i, a, b }) - 1;
        // The cells along the segment (a long diagonal's bounding box would
        // cover far too many).
        const cells = new Set(), steps = Math.ceil(distance(a, b) / (BUCKET / 2)) + 1;
        for (let k = 0; k <= steps; k++) {
          const x = a.x + (b.x - a.x) * k / steps, y = a.y + (b.y - a.y) * k / steps;
          for (const [dx, dy] of [[0, 0], [1, 0], [-1, 0], [0, 1], [0, -1]]) cells.add(`${Math.floor(x / BUCKET) + dx}:${Math.floor(y / BUCKET) + dy}`);
        }
        for (const cell of cells) push(buckets, cell, index);
      }
    });
    const near = (box) => {
      const found = new Set();
      for (let bx = Math.floor(box.x / BUCKET); bx <= Math.floor((box.x + box.width) / BUCKET); bx++) {
        for (let by = Math.floor(box.y / BUCKET); by <= Math.floor((box.y + box.height) / BUCKET); by++) {
          for (const index of buckets.get(`${bx}:${by}`) || []) found.add(index);
        }
      }
      return [...found].sort((p, q) => p - q).map((index) => segments[index]);
    };
    return { segments, buckets, near };
  }
  function intersection(s, u) {
    const r = { x: s.b.x - s.a.x, y: s.b.y - s.a.y }, q = { x: u.b.x - u.a.x, y: u.b.y - u.a.y };
    const d = r.x * q.y - r.y * q.x;
    if (Math.abs(d) < EPS) return null;
    const w = { x: u.a.x - s.a.x, y: u.a.y - s.a.y };
    const t = (w.x * q.y - w.y * q.x) / d, v = (w.x * r.y - w.y * r.x) / d;
    if (t <= 0.001 || t >= 0.999 || v <= 0.001 || v >= 0.999) return null;
    return { x: s.a.x + r.x * t, y: s.a.y + r.y * t };
  }
  // Where a horizontal run crosses a trace in another direction, it hops
  // over it with a bridge whose ramps keep the mode's angles. Every trace is
  // also drawn over a casing of the sheet's colour, so a later trace cuts a
  // small gap into an earlier one wherever they cross.
  function addHops(traces, ramp, halfWidths) {
    const trunkOf = traces.map((trace) => trace.trunk || null);
    const kin = (a, b) => a === b || (trunkOf[a] !== null && trunkOf[a] === trunkOf[b]);
    const segments = [];
    traces.forEach((trace, ti) => { for (let i = 0; i + 1 < trace.core.length; i++) segments.push({ ti, i, a: trace.core[i], b: trace.core[i + 1] }); });
    const hops = new Map(); // segment index → [{ at: distance along it, half: half width of the trace crossed }]
    const horizontal = (s) => Math.abs(s.a.y - s.b.y) < 0.01;
    const footprint = (s) => HOP.flat + 2 * (halfWidths[s.ti] || 0) + 2 * HOP.height / Math.tan(ramp * Math.PI / 180);
    const fits = (s, at) => { const along = distance(s.a, at), length = distance(s.a, s.b); return along > footprint(s) / 2 + 1 && length - along > footprint(s) / 2 + 3; };
    // Horizontal runs in a grid of cells (a band of 24 px by 96 px); each
    // other segment walks the cells along its course. Only such pairs hop,
    // so no other pair is compared.
    const CELL_X = 96, CELL_Y = 24;
    const cellKey = (cx, cy) => (cx + 50000) * 100000 + (cy + 50000);
    const cells = new Map();
    // A trunk's runs are drawn as one thick trunk over them, so they do not hop.
    segments.forEach((s, index) => {
      if (!horizontal(s) || trunkOf[s.ti] !== null) return;
      const cy = Math.floor(s.a.y / CELL_Y);
      for (let cx = Math.floor(Math.min(s.a.x, s.b.x) / CELL_X); cx <= Math.floor(Math.max(s.a.x, s.b.x) / CELL_X); cx++) push(cells, cellKey(cx, cy), index);
    });
    segments.forEach((u) => {
      if (horizontal(u)) return;
      const y1 = Math.min(u.a.y, u.b.y), y2 = Math.max(u.a.y, u.b.y);
      const seen = new Set();
      for (let cy = Math.floor(y1 / CELL_Y); cy <= Math.floor(y2 / CELL_Y); cy++) {
        // The segment's x range within this band of rows.
        const ya = Math.max(y1, cy * CELL_Y), yb = Math.min(y2, (cy + 1) * CELL_Y);
        const xAt = (y) => u.a.x + (u.b.x - u.a.x) * (y - u.a.y) / (u.b.y - u.a.y);
        const xa = Math.min(xAt(ya), xAt(yb)), xb = Math.max(xAt(ya), xAt(yb));
        for (let cx = Math.floor(xa / CELL_X); cx <= Math.floor(xb / CELL_X); cx++) {
          for (const i of cells.get(cellKey(cx, cy)) || []) {
            if (seen.has(i)) continue;
            seen.add(i);
            const s = segments[i];
            if (kin(s.ti, u.ti)) continue;
            const at = intersection(s, u);
            if (at && fits(s, at)) push(hops, i, { at: distance(s.a, at), half: halfWidths[u.ti] || 0 });
          }
        }
      }
    });
    let index = 0;
    traces.forEach((trace, ti) => {
      const points = trace.core, out = [points[0]];
      for (let i = 0; i + 1 < points.length; i++, index++) {
        const list = (hops.get(index) || []).sort((p, q) => p.at - q.at || p.half - q.half);
        const a = points[i], b = points[i + 1], length = distance(a, b);
        const d = { x: (b.x - a.x) / length, y: (b.y - a.y) / length };
        let n = { x: d.y, y: -d.x };
        if (n.y > EPS || (Math.abs(n.y) <= EPS && n.x > 0)) n = { x: -n.x, y: -n.y };
        const own = HOP.flat / 2 + (halfWidths[ti] || 0), run = HOP.height / Math.tan(ramp * Math.PI / 180);
        // Hops close together merge into one longer bridge; a bridge over a
        // trunk is as much wider as the trunk.
        const spans = [];
        for (const { at, half: other } of list) {
          const last = spans[spans.length - 1];
          if (last && at - last[1] < 2 * (own + last[2] + run) + 2) { last[1] = at; last[2] = Math.max(last[2], other); } else spans.push([at, at, other]);
        }
        let floor = 0;
        for (const [from, to, other] of spans) {
          const half = own + other;
          const s0 = from - half - run, s1 = to + half + run;
          if (s0 < floor + 1 || s1 > length - 3) continue;
          const at = (t, lift) => ({ x: a.x + d.x * t + n.x * lift, y: a.y + d.y * t + n.y * lift });
          out.push(at(s0, 0), at(from - half, HOP.height), at(to + half, HOP.height), at(s1, 0));
          floor = s1;
        }
        out.push(b);
      }
      trace.points = out;
    });
  }

  // ------------------------------------------------------------ labels
  function boxHitsSegment(box, a, b) {
    let t0 = 0, t1 = 1;
    const dx = b.x - a.x, dy = b.y - a.y;
    for (const [p, q] of [[-dx, a.x - box.x], [dx, box.x + box.width - a.x], [-dy, a.y - box.y], [dy, box.y + box.height - a.y]]) {
      if (Math.abs(p) < EPS) { if (q < 0) return false; continue; }
      const r = q / p;
      if (p < 0) { if (r > t1) return false; if (r > t0) t0 = r; } else { if (r < t0) return false; if (r < t1) t1 = r; }
    }
    return true;
  }
  const overlaps = (a, b) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
  // A label sits on a horizontal run of its own trace, in a spot that covers
  // no card, no other label and no other trace; a label that fits nowhere is
  // marked crowded and shown only when its edge is highlighted.
  function placeLabels(traces, positions, sizes, taken) {
    if (!sizes.some(Boolean)) { for (const trace of traces) trace.label = { ...longestMiddle(trace.core), crowded: true }; return; }
    const index = segmentIndex(traces, "points");
    const cards = [...positions.values()].map((box) => ({ x: box.x - 4 - (box.hex || 0), y: box.y - 4, width: box.width + 8 + 2 * (box.hex || 0), height: box.height + 8 }));
    const order = sizes.map((size, i) => i).filter((i) => sizes[i]).sort((p, q) => sizes[q].priority - sizes[p].priority || p - q);
    for (const ti of order) {
      const { width, height } = sizes[ti];
      const points = traces[ti].core;
      const runs = [];
      for (let i = 0; i + 1 < points.length; i++) {
        const a = points[i], b = points[i + 1];
        if (Math.abs(a.y - b.y) < 0.01 && Math.abs(b.x - a.x) >= width + 12) runs.push({ a, b, length: Math.abs(b.x - a.x), i });
      }
      runs.sort((p, q) => q.length - p.length || p.i - q.i);
      let found = null;
      for (const run of runs) {
        for (const f of [0.5, 0.3, 0.7, 0.18, 0.82]) {
          const x = run.a.x + (run.b.x - run.a.x) * f;
          if (Math.abs(x - run.a.x) < width / 2 + 4 || Math.abs(run.b.x - x) < width / 2 + 4) continue;
          const box = { x: x - width / 2, y: run.a.y - height / 2, width, height };
          if (cards.some((card) => overlaps(card, box)) || taken.some((other) => overlaps(other, box))) continue;
          if (index.near(box).some((s) => s.ti !== ti && boxHitsSegment(box, s.a, s.b))) continue;
          found = { x, y: run.a.y, box };
          break;
        }
        if (found) break;
      }
      if (found) { taken.push(found.box); traces[ti].label = { x: found.x, y: found.y, crowded: false }; }
      else traces[ti].label = { ...longestMiddle(points), crowded: true };
    }
    traces.forEach((trace, i) => { if (!trace.label) trace.label = { ...longestMiddle(trace.core), crowded: !sizes[i] }; });
  }
  function longestMiddle(points) {
    let best = null;
    for (let i = 0; i + 1 < points.length; i++) {
      const length = distance(points[i], points[i + 1]) + (Math.abs(points[i].y - points[i + 1].y) < 0.01 ? 1000 : 0);
      if (!best || length > best.length) best = { length, x: (points[i].x + points[i + 1].x) / 2, y: (points[i].y + points[i + 1].y) / 2 };
    }
    return { x: best.x, y: best.y };
  }

  // ------------------------------------------------------------ trunks
  // Each trunk's traces cut into pieces at every point of the others, and
  // the number of its traces running along each piece. Runs are the longest
  // stretches of a trace whose pieces are shared by at least a tier's
  // count, drawn in that tier's width; dots mark where a trace joins.
  function trunkRuns(traces) {
    const groups = new Map();
    traces.forEach((trace, ti) => { if (trace.trunk) push(groups, trace.trunk, ti); });
    const key = (p) => `${p.x},${p.y}`;
    const between = (v, a, b) => v > Math.min(a, b) + 0.05 && v < Math.max(a, b) - 0.05;
    const trunks = [];
    for (const id of [...groups.keys()].sort()) {
      const members = groups.get(id);
      const byY = new Map(), byX = new Map(), seen = new Set();
      for (const ti of members) {
        for (const p of traces[ti].core) {
          if (seen.has(key(p))) continue;
          seen.add(key(p));
          push(byY, p.y, p);
          push(byX, p.x, p);
        }
      }
      const paths = members.map((ti) => {
        const core = traces[ti].core, pieces = [];
        for (let i = 0; i + 1 < core.length; i++) {
          const a = core[i], b = core[i + 1];
          const cuts = a.y === b.y ? (byY.get(a.y) || []).filter((p) => between(p.x, a.x, b.x))
            : a.x === b.x ? (byX.get(a.x) || []).filter((p) => between(p.y, a.y, b.y)) : [];
          cuts.sort((p, q) => distance(a, p) - distance(a, q));
          let from = a;
          for (const p of [...cuts, b]) { if (distance(from, p) >= 0.05) { pieces.push([from, p]); from = p; } }
        }
        return pieces;
      });
      const pieceKey = ([p, q]) => { const s = key(p), t = key(q); return s < t ? `${s};${t}` : `${t};${s}`; };
      const count = new Map();
      for (const pieces of paths) for (const k of new Set(pieces.map(pieceKey))) count.set(k, (count.get(k) || 0) + 1);
      const shared = (piece) => count.get(pieceKey(piece)) || 0;
      const tiers = [];
      for (const tier of TRUNK_TIERS) {
        const runs = new Map();
        for (const pieces of paths) {
          let run = null;
          const flush = () => { if (run) runs.set(run.map(key).join(" "), run); run = null; };
          for (const piece of pieces) {
            if (shared(piece) >= tier.min) { if (!run) run = [piece[0]]; run.push(piece[1]); } else flush();
          }
          flush();
        }
        if (runs.size) tiers.push({ ...tier, runs: [...runs.keys()].sort().map((k) => runs.get(k)) });
      }
      const dots = new Map();
      for (const pieces of paths) {
        const at = pieces.findIndex((piece) => shared(piece) >= 2);
        if (at > 0) dots.set(key(pieces[at][0]), pieces[at][0]);
      }
      const core = traces[members[0]].core;
      trunks.push({ id, members, tiers, dots: [...dots.keys()].sort().map((k) => dots.get(k)), end: core[core.length - 1],
        width: tiers.length ? tiers[tiers.length - 1].width : 0 });
    }
    return trunks;
  }
  // A trunk's tag sits on the middle of one of its widest runs, preferring
  // a long horizontal stretch, clear of cards and other labels.
  function placeTags(trunks, sizes, cards, taken) {
    for (const trunk of trunks) {
      const size = sizes && sizes.get(trunk.id);
      const top = trunk.tiers[trunk.tiers.length - 1];
      if (!size || !top) { trunk.label = null; continue; }
      const segments = [];
      for (const run of top.runs) for (let i = 0; i + 1 < run.length; i++) segments.push({ a: run[i], b: run[i + 1], flat: Math.abs(run[i].y - run[i + 1].y) < 0.01 });
      segments.sort((p, q) => (q.flat - p.flat) || distance(q.a, q.b) - distance(p.a, p.b) || p.a.y - q.a.y || p.a.x - q.a.x);
      let found = null;
      for (const segment of segments) {
        for (const f of [0.5, 0.3, 0.7, 0.15, 0.85]) {
          const x = segment.a.x + (segment.b.x - segment.a.x) * f, y = segment.a.y + (segment.b.y - segment.a.y) * f;
          const box = { x: x - size.width / 2, y: y - size.height / 2, width: size.width, height: size.height };
          if (cards.some((card) => overlaps(card, box)) || taken.some((other) => overlaps(other, box))) continue;
          found = { x, y, box };
          break;
        }
        if (found) break;
      }
      if (found) taken.push(found.box);
      const fallback = segments[0] ? { x: (segments[0].a.x + segments[0].b.x) / 2, y: (segments[0].a.y + segments[0].b.y) / 2 } : trunk.end;
      trunk.label = found ? { x: round(found.x), y: round(found.y), crowded: false } : { x: round(fallback.x), y: round(fallback.y), crowded: true };
    }
  }

  function finish(input, positions, traces, notes, grid, ramp) {
    for (const trace of traces) trace.core = trace.core.map((p) => ({ x: round(p.x), y: round(p.y) }));
    const trunks = trunkRuns(traces);
    const halfWidths = input.edges.map((edge) => edge.halfWidth || 0);
    for (const trunk of trunks) for (const ti of trunk.members) halfWidths[ti] = Math.max(halfWidths[ti], trunk.width / 2);
    addHops(traces, ramp, halfWidths);
    for (const trace of traces) trace.points = trace.points.map((p) => ({ x: round(p.x), y: round(p.y) }));
    // A trunk's traces are labelled by its tag; their own labels show on hover.
    const sizes = (input.labels || traces.map(() => null)).map((size, i) => (traces[i].trunk ? null : size));
    const cards = [...positions.values()].map((box) => ({ x: box.x - 4 - (box.hex || 0), y: box.y - 4, width: box.width + 8 + 2 * (box.hex || 0), height: box.height + 8 }));
    const taken = [];
    placeTags(trunks, input.tags ? new Map(trunks.map((trunk) => [trunk.id, input.tags(trunk.members)])) : null, cards, taken);
    placeLabels(traces, positions, sizes, taken);
    let extent = null;
    const add = (x, y) => { extent = extent ? { x1: Math.min(extent.x1, x), y1: Math.min(extent.y1, y), x2: Math.max(extent.x2, x), y2: Math.max(extent.y2, y) } : { x1: x, y1: y, x2: x, y2: y }; };
    for (const box of positions.values()) { add(box.x - (box.hex || 0), box.y); add(box.x + box.width + (box.hex || 0), box.y + box.height); }
    for (const trace of traces) for (const p of trace.points) add(p.x, p.y);
    return { positions, traces, trunks, notes, grid, extent };
  }

  // ------------------------------------------------------------ public
  function route(input) {
    return input.mode === "hex" ? routeHex(input) : routePcb(input);
  }
  // The grid cell under a point, for dropping a dragged card: one row or
  // slot beyond the grid is allowed, never a negative one.
  function nearest(centres, value, step) {
    let best = 0;
    centres.forEach((centre, i) => { if (Math.abs(centre - value) < Math.abs(centres[best] - value)) best = i; });
    const last = centres.length - 1;
    if (value > centres[last] + step / 2) return last + 1;
    return best;
  }
  function snap(grid, x, y) {
    const row = nearest(grid.rowY.map((top) => top + grid.height / 2), y, grid.rowPitch);
    if (grid.mode === "hex") {
      const shift = (row % 2) * 0.5;
      const col = Math.max(0, Math.min(grid.C, Math.round((x - ORIGIN.x - grid.ear - grid.W / 2) / grid.P - shift)));
      return { row, col };
    }
    return { row, col: nearest(grid.colX.map((left, c) => left + grid.colW[c] / 2), x, grid.colPitch) };
  }
  function slotBox(grid, row, col) {
    const top = row < grid.rowY.length ? grid.rowY[row] : grid.rowY[grid.rowY.length - 1] + grid.rowPitch * (row - grid.rowY.length + 1);
    if (grid.mode === "hex") return { x: ORIGIN.x + grid.ear + (col + (row % 2) * 0.5) * grid.P, y: top, width: grid.W, height: grid.height, hex: grid.ear };
    const left = col < grid.colX.length ? grid.colX[col] : grid.colX[grid.colX.length - 1] + grid.colW[grid.colX.length - 1] + MIN_GAP_X;
    return { x: left, y: top, width: col < grid.colW.length ? grid.colW[col] : grid.colPitch - MIN_GAP_X, height: grid.height };
  }
  // A polyline moved sideways by `offset` (positive: to the left of its
  // direction), with mitred joins: the strands of a bus.
  function offsetPolyline(points, offset) {
    const unit = (p, q) => { const d = distance(p, q) || 1; return { x: (q.x - p.x) / d, y: (q.y - p.y) / d }; };
    const out = [];
    for (let i = 0; i < points.length; i++) {
      const prev = i > 0 ? unit(points[i - 1], points[i]) : null;
      const next = i + 1 < points.length ? unit(points[i], points[i + 1]) : null;
      const n1 = prev && { x: -prev.y, y: prev.x }, n2 = next && { x: -next.y, y: next.x };
      if (!n1 || !n2) { const n = n1 || n2; out.push({ x: points[i].x + n.x * offset, y: points[i].y + n.y * offset }); continue; }
      const m = { x: n1.x + n2.x, y: n1.y + n2.y }, length = Math.hypot(m.x, m.y);
      if (length < EPS) { out.push({ x: points[i].x + n1.x * offset, y: points[i].y + n1.y * offset }); continue; }
      const scale = Math.min(4, 1 / ((m.x * n1.x + m.y * n1.y) / length)) * offset / length;
      out.push({ x: points[i].x + m.x * scale, y: points[i].y + m.y * scale });
    }
    return out;
  }
  function pathData(points) {
    return points.map((p, i) => `${i ? "L" : "M"} ${round(p.x)} ${round(p.y)}`).join(" ");
  }
  // The outline of a hexagonal card at (0, 0).
  function hexOutline(width, height) {
    const ear = earOf(height);
    return `M 0 0 H ${round(width)} L ${round(width + ear)} ${round(height / 2)} L ${round(width)} ${round(height)} H 0 L ${round(-ear)} ${round(height / 2)} Z`;
  }
  return { route, snap, slotBox, offsetPolyline, pathData, hexOutline, earOf, PITCH, TRUNK_TIERS };
})();
