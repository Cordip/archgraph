"use strict";

// The read-only code viewer in the inspector's second pane. It shows one
// mapped file from GET /api/source, marks the lines a piece of evidence
// points at and scrolls to them.
//
// Only the rows in view (and a margin) are elements, so a file of tens of
// thousands of lines opens and scrolls as fast as a short one. Syntax
// colouring is a small hand-written scanner per line, with no regular
// expressions: it runs once over the file to know where block comments and
// multi-line strings start, then only for the rows being drawn.
//
// Built with createElement and textContent only; sizes and positions go
// through element.style (CSSOM), which the server's CSP allows.
const Viewer = (() => {
  const OVERSCAN = 40;
  const PLAIN_LINE = 2000; // longer lines (minified code) are not coloured
  const MARK_LIMIT = 200;

  function el(tag, className, text) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    if (text !== undefined && text !== null) element.textContent = String(text);
    return element;
  }

  // ------------------------------------------------------------ paths
  // The server decides what it serves; this refuses, without asking it,
  // what can never be a repository-relative file path.
  function unsafePath(path) {
    if (typeof path !== "string" || !path) return "no file path";
    if (path.startsWith("package:")) return "an imported package, not a repository file";
    if (path.startsWith("/") || path.startsWith("\\") || path[1] === ":") return "an absolute path; only repository-relative paths are shown";
    if (path.includes("\\") || path.includes("\0") || path.includes("\n")) return "a path with a backslash or control character";
    if (path.split("/").some((part) => part === ".." || part === "." || part === "")) return "a path with `..`, `.` or an empty segment";
    return null;
  }

  // ------------------------------------------------------------ languages
  const words = (text) => new Set(text.split(" "));
  const C_LIKE = { line: ["//"], block: ["/*", "*/"] };
  const LANGUAGES = {
    rust: { ...C_LIKE, strings: { "\"": true }, rustChars: true, raw: true,
      keywords: words("as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while Some None Ok Err") },
    js: { ...C_LIKE, strings: { "\"": false, "'": false, "`": true },
      keywords: words("abstract as async await break case catch class const continue debugger declare default delete do else enum export extends false finally for from function get if implements import in instanceof interface let new null of private protected public readonly return set static super switch this throw true try type typeof undefined var void while with yield") },
    python: { line: ["#"], strings: { "\"": false, "'": false }, triple: true,
      keywords: words("and as assert async await break class continue def del elif else except False finally for from global if import in is lambda match None nonlocal not or pass raise return self True try while with yield") },
    ruby: { line: ["#"], strings: { "\"": false, "'": false },
      keywords: words("alias and begin break case class def defined? do else elsif end ensure false for if in module next nil not or redo require require_relative rescue retry return self super then true undef unless until when while yield include extend attr_reader attr_accessor") },
    css: { block: ["/*", "*/"], strings: { "\"": false, "'": false }, css: true, keywords: words("") },
  };
  // Markup (see scanMarkup): a Vue component is HTML whose <script> and
  // <style> hold TypeScript and CSS; ERB is HTML with Ruby in <% %>.
  LANGUAGES.vue = { markup: true, sections: { script: LANGUAGES.js, style: LANGUAGES.css } };
  LANGUAGES.erb = { markup: true, erb: true, sections: {} };
  const EXTENSIONS = { rs: "rust", ts: "js", tsx: "js", js: "js", jsx: "js", mjs: "js", cjs: "js", mts: "js", cts: "js",
    py: "python", pyi: "python", rb: "ruby", rake: "ruby", css: "css", scss: "css", less: "css", vue: "vue", erb: "erb" };
  function languageOf(path) {
    const dot = path.lastIndexOf(".");
    return LANGUAGES[EXTENSIONS[dot < 0 ? "" : path.slice(dot + 1).toLowerCase()]] || null;
  }
  const isIdentStart = (c) => (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || c === "_" || c === "$" || c > "\x7f";
  const isIdent = (c) => isIdentStart(c) || (c >= "0" && c <= "9");
  const isDigit = (c) => c >= "0" && c <= "9";

  // Scans one line. `state` is what the previous line left open: null, or
  // { end, kind } for a block comment or a string that runs on. Pushes
  // [start, end, class] spans to `out` (when given) and returns the state
  // the next line starts in. Every loop step advances, so the scan is
  // linear in the line's length.
  function scanLine(line, state, lang, out) {
    if (lang.markup) return scanMarkup(line, state, lang, out);
    const push = (start, end, kind) => { if (out && end > start) out.push([start, end, kind]); };
    let i = 0;
    const n = line.length;
    // Finds `end` from `from`, honouring backslash escapes in strings.
    const close = (from, end, escapes) => {
      for (let j = from; j < n; j++) {
        if (escapes && line[j] === "\\") { j++; continue; }
        if (line.startsWith(end, j)) return j + end.length;
      }
      return -1;
    };
    if (state) {
      const stop = close(0, state.end, state.kind === "string");
      if (stop < 0) { push(0, n, state.kind); return state; }
      push(0, stop, state.kind);
      i = stop;
    }
    while (i < n) {
      const c = line[i];
      if (lang.line && lang.line.some((start) => line.startsWith(start, i))) { push(i, n, "comment"); return null; }
      if (lang.block && line.startsWith(lang.block[0], i)) {
        const stop = close(i + 2, lang.block[1], false);
        if (stop < 0) { push(i, n, "comment"); return { end: lang.block[1], kind: "comment" }; }
        push(i, stop, "comment");
        i = stop;
        continue;
      }
      if (lang.triple && (line.startsWith("\"\"\"", i) || line.startsWith("'''", i))) {
        const quote = line.slice(i, i + 3);
        const stop = close(i + 3, quote, true);
        if (stop < 0) { push(i, n, "string"); return { end: quote, kind: "string" }; }
        push(i, stop, "string");
        i = stop;
        continue;
      }
      if (lang.raw && c === "r" && (line[i + 1] === "#" || line[i + 1] === "\"") && (i === 0 || !isIdent(line[i - 1]))) {
        let hashes = 0;
        while (line[i + 1 + hashes] === "#") hashes++;
        if (line[i + 1 + hashes] === "\"") {
          const end = "\"" + "#".repeat(hashes);
          const stop = close(i + 2 + hashes, end, false);
          if (stop < 0) { push(i, n, "string"); return { end, kind: "raw" }; }
          push(i, stop, "string");
          i = stop;
          continue;
        }
      }
      if (lang.rustChars && c === "'") {
        // 'a' or '\n' is a character; 'a without a closing quote a lifetime.
        const stop = line[i + 1] === "\\" ? close(i + 2, "'", true) : line[i + 2] === "'" ? i + 3 : -1;
        if (stop > 0) { push(i, stop, "string"); i = stop; continue; }
        let j = i + 1;
        while (j < n && isIdent(line[j])) j++;
        push(i, j, "type");
        i = Math.max(j, i + 1);
        continue;
      }
      if (Object.prototype.hasOwnProperty.call(lang.strings, c)) {
        const multi = lang.strings[c];
        const stop = close(i + 1, c, true);
        if (stop < 0) { push(i, n, "string"); return multi ? { end: c, kind: "string" } : null; }
        push(i, stop, "string");
        i = stop;
        continue;
      }
      if (isDigit(c) || (c === "#" && lang.css)) {
        let j = i + 1;
        while (j < n && (isIdent(line[j]) || line[j] === ".")) j++;
        push(i, j, "number");
        i = j;
        continue;
      }
      if (c === "@" && lang.css) {
        let j = i + 1;
        while (j < n && (isIdent(line[j]) || line[j] === "-")) j++;
        push(i, j, "keyword");
        i = j;
        continue;
      }
      if (isIdentStart(c)) {
        let j = i + 1;
        while (j < n && (isIdent(line[j]) || (lang.css && line[j] === "-"))) j++;
        if (line[j] === "?" && lang === LANGUAGES.ruby && lang.keywords.has(line.slice(i, j + 1))) j++;
        const word = line.slice(i, j);
        if (lang.keywords.has(word)) push(i, j, "keyword");
        else if (!lang.css && c >= "A" && c <= "Z") push(i, j, "type");
        i = j;
        continue;
      }
      i++;
    }
    return null;
  }
  // Scans a line of HTML-like markup, as lightly as the other languages:
  // tag names are keywords, attribute values strings, <!-- --> comments
  // comments. Text between tags is left plain. Two kinds of holes are
  // handed to another language's scanLine, with their spans shifted back
  // onto the line:
  // - the raw text of a <script> or <style> element (Vue: `lang.sections`),
  //   up to its closing tag, as browsers read it;
  // - ERB's <% %> (and <%= %>, <%- %>), Ruby up to the next `%>` wherever
  //   it starts, even inside an attribute value or a comment, since ERB
  //   runs before the HTML is read; <%# %> is a comment. The delimiters are
  //   marked as types so that the Ruby stands out from the page.
  // The state a line leaves is null in plain text, else an object that
  // says where the line ended: in a tag, a quoted value, a comment, a
  // section (with the inner language's state) or a Ruby hole (with the
  // state to return to at `%>`).
  function scanMarkup(line, state, lang, out) {
    // A span that continues the previous one of its kind extends it.
    const push = (start, end, kind) => {
      if (!out || end <= start) return;
      const last = out[out.length - 1];
      if (last && last[1] === start && last[2] === kind) last[1] = end; else out.push([start, end, kind]);
    };
    const embedded = (inner, from, to, start) => {
      const spans = out ? [] : null;
      const next = scanLine(line.slice(from, to), start, inner, spans);
      if (spans) for (const [a, b, kind] of spans) out.push([a + from, b + from, kind]);
      return next;
    };
    // The first of `needle` and, in ERB, a Ruby hole: [index, isHole].
    const nextStop = (needle, from) => {
      const at = line.indexOf(needle, from), hole = lang.erb ? line.indexOf("<%", from) : -1;
      return hole >= 0 && (at < 0 || hole < at) ? [hole, true] : [at, false];
    };
    let { mode = "text", quote = "", tag = "", section = "", inner = null, remark = false, back = null } = state || {};
    const n = line.length;
    let i = 0;
    while (i < n) {
      if (mode === "section") {
        const close = line.indexOf(`</${section}`, i);
        inner = embedded(lang.sections[section], i, close < 0 ? n : close, inner);
        if (close < 0) break;
        mode = "text"; section = ""; inner = null; i = close;
        continue;
      }
      if (mode === "ruby") {
        const close = line.indexOf("%>", i);
        if (remark) push(i, close < 0 ? n : close, "comment");
        else inner = embedded(LANGUAGES.ruby, i, close < 0 ? n : close, inner);
        if (close < 0) break;
        push(close, close + 2, "type");
        ({ mode, quote, tag } = back);
        back = null; inner = null; remark = false; i = close + 2;
        continue;
      }
      if (lang.erb && line.startsWith("<%", i)) {
        let j = i + 2;
        remark = line[j] === "#";
        if (line[j] === "=" || line[j] === "-" || line[j] === "#") j++;
        push(i, j, "type");
        back = { mode, quote, tag };
        mode = "ruby"; inner = null; i = j;
        continue;
      }
      if (mode === "comment" || mode === "value") {
        const kind = mode === "comment" ? "comment" : "string", end = mode === "comment" ? "-->" : quote;
        const [at, hole] = nextStop(end, i);
        if (at < 0) { push(i, n, kind); break; }
        if (hole) { push(i, at, kind); i = at; continue; }
        push(i, at + end.length, kind);
        i = at + end.length;
        mode = mode === "comment" ? "text" : "tag";
        continue;
      }
      if (mode === "tag") {
        const c = line[i];
        if (c === "\"" || c === "'") { push(i, i + 1, "string"); mode = "value"; quote = c; i++; continue; }
        if (c === ">") {
          mode = "text";
          if (Object.prototype.hasOwnProperty.call(lang.sections, tag) && line[i - 1] !== "/") { mode = "section"; section = tag; inner = null; }
          tag = "";
        }
        i++;
        continue;
      }
      // Plain text: on to the next tag, comment or hole.
      const at = line.indexOf("<", i);
      if (at < 0) break;
      i = at;
      if (lang.erb && line.startsWith("<%", i)) continue;
      if (line.startsWith("<!--", i)) { push(i, i + 4, "comment"); mode = "comment"; i += 4; continue; }
      let j = i + 1;
      const closing = line[j] === "/";
      if (closing || line[j] === "!") j++;
      if (!(line[j] >= "a" && line[j] <= "z") && !(line[j] >= "A" && line[j] <= "Z")) { i++; continue; }
      const nameStart = j;
      while (j < n && (isIdent(line[j]) || line[j] === "-" || line[j] === ":" || line[j] === ".")) j++;
      push(i, j, "keyword");
      mode = "tag";
      tag = closing ? "" : line.slice(nameStart, j).toLowerCase();
      i = j;
    }
    return mode === "text" ? null : { mode, quote, tag, section, inner, remark, back };
  }
  // The state each line starts in, from one pass over the file.
  function lineStates(lines, lang) {
    const states = new Array(lines.length);
    let state = null;
    for (let i = 0; i < lines.length; i++) {
      states[i] = state;
      state = lines[i].length > PLAIN_LINE ? state : scanLine(lines[i], state, lang, null);
    }
    return states;
  }

  // ------------------------------------------------------------ references
  // GitNexus reports file-to-file dependencies without lines. The lines of
  // `from` that name `to` are found in the text, deterministically: first
  // import-like lines whose module specifier ends with the target's path
  // (`../domain/model`, `domain.model`, `crate::domain::model`); if none,
  // lines naming the target's file name or its CamelCase as a whole word
  // (`user_session` or `UserSession`, as Rails code refers to a file).
  const INDEX_NAMES = new Set(["index", "__init__", "mod", "_index"]);
  const SPEC_EXTENSIONS = new Set(["js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts", "py", "rb", "rs", "css", "scss", "less", "vue", "svelte", "json"]);
  const IMPORT_STARTS = ["import", "from ", "export ", "require", "use ", "pub use ", "pub(crate) use ", "mod ", "pub mod ", "include ", "extend ", "@import", "@use", "@forward", "load "];
  function stripExtension(path) {
    const slash = path.lastIndexOf("/"), dot = path.lastIndexOf(".");
    return dot > slash + 1 && SPEC_EXTENSIONS.has(path.slice(dot + 1).toLowerCase()) ? path.slice(0, dot) : path;
  }
  function camel(name) {
    return name.split(/[_-]/).filter(Boolean).map((part) => part[0].toUpperCase() + part.slice(1)).join("");
  }
  function targetTerms(to) {
    const bare = stripExtension(to);
    const parts = bare.split("/");
    let name = parts[parts.length - 1];
    const paths = [bare];
    if (INDEX_NAMES.has(name) && parts.length > 1) { paths.push(parts.slice(0, -1).join("/")); name = parts[parts.length - 2]; }
    const names = [...new Set([name, camel(name)])].filter((item) => item.length >= 3);
    return { paths, names, name };
  }
  // A module specifier as a path: `./x`, `../x`, `@/x` and `~/x` prefixes
  // dropped, a known extension removed, dots of dotted modules as slashes.
  function specifierPath(word) {
    let spec = word;
    for (;;) {
      if (spec.startsWith("./")) spec = spec.slice(2);
      else if (spec.startsWith("../")) spec = spec.slice(3);
      else if (spec.startsWith("@/") || spec.startsWith("~/")) spec = spec.slice(2);
      else if (spec.startsWith("crate/") || spec.startsWith("self/")) spec = spec.slice(spec.indexOf("/") + 1);
      else if (spec.startsWith("super/")) spec = spec.slice(6);
      else break;
    }
    spec = stripExtension(spec);
    while (spec.startsWith(".")) spec = spec.slice(1);
    return spec.includes("/") ? spec : spec.split(".").join("/");
  }
  const SPEC_CHARS = (c) => isIdent(c) || c === "." || c === "/" || c === "-" || c === "@" || c === "~";
  function importNames(line, paths) {
    const trimmed = line.trimStart();
    if (!IMPORT_STARTS.some((start) => trimmed.startsWith(start)) && !line.includes("require(") && !line.includes("import(")) return false;
    const text = line.split("::").join("/");
    for (let i = 0; i < text.length;) {
      if (!SPEC_CHARS(text[i])) { i++; continue; }
      let j = i;
      while (j < text.length && SPEC_CHARS(text[j])) j++;
      const spec = specifierPath(text.slice(i, j));
      i = j;
      if (!spec || spec === "crate" || spec === "self" || spec === "super") continue;
      // `use a::b::Thing` and `from a.b import c` name a module, then maybe
      // a symbol in it: a shorter prefix of two or more parts also counts.
      const parts = spec.split("/");
      for (let keep = parts.length; keep >= Math.min(parts.length, 2) && keep >= parts.length - 2; keep--) {
        const prefix = parts.slice(0, keep).join("/");
        if (paths.some((path) => path === prefix || path.endsWith(`/${prefix}`))) return true;
      }
    }
    return false;
  }
  function namesWord(line, word) {
    for (let at = line.indexOf(word); at >= 0; at = line.indexOf(word, at + 1)) {
      const before = at ? line[at - 1] : "", after = line[at + word.length] || "";
      if (!(before && isIdent(before)) && !(after && isIdent(after))) return true;
    }
    return false;
  }
  function referenceLines(lines, to) {
    const terms = targetTerms(to);
    const found = [];
    for (let i = 0; i < lines.length && found.length < MARK_LIMIT; i++) if (importNames(lines[i], terms.paths)) found.push(i + 1);
    if (found.length) return { how: "import", lines: found, terms };
    for (let i = 0; i < lines.length && found.length < MARK_LIMIT; i++) if (terms.names.some((name) => namesWord(lines[i], name))) found.push(i + 1);
    return { how: found.length ? "name" : "none", lines: found, terms };
  }

  // ------------------------------------------------------------ view
  // hooks: { fetch(url) → payload or throws { message, reason },
  //          layout() after the panel's shape changed }
  function create(pane, splitter, inspector, hooks) {
    const stored = () => { try { return JSON.parse(window.localStorage.getItem("archgraph.viewer.v1")) || {}; } catch { return {}; } };
    const save = (value) => { try { window.localStorage.setItem("archgraph.viewer.v1", JSON.stringify(value)); } catch { /* not remembered */ } };
    let split = clamp(Number(stored().split) || 0.4);
    let file = null; // { path, text, lines, states, lang, hash, node, request }
    let marks = { lines: [], exact: false, note: "", at: -1 };
    let rendered = { first: -1, last: -1 };
    let lineHeight = 20;
    let frame = 0;
    let token = 0;

    const head = el("div", "pane-head viewer-head");
    const title = el("h2", "viewer-title");
    title.id = "viewer-title";
    const dir = el("span", "viewer-dir"), base = el("span", "viewer-base");
    title.append(dir, base);
    const copy = el("button", "text-button viewer-copy", "Copy path");
    copy.type = "button";
    const closeButton = el("button", "icon-button");
    closeButton.type = "button";
    closeButton.id = "viewer-close";
    closeButton.setAttribute("aria-label", "Close the source viewer");
    const cross = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    cross.setAttribute("viewBox", "0 0 20 20");
    cross.setAttribute("aria-hidden", "true");
    const crossPath = document.createElementNS("http://www.w3.org/2000/svg", "path");
    crossPath.setAttribute("d", "M5 5l10 10M15 5L5 15");
    cross.append(crossPath);
    closeButton.append(cross);
    head.append(title, copy, closeButton);
    const facts = el("p", "viewer-facts");
    const stale = el("div", "viewer-stale");
    stale.hidden = true;
    stale.setAttribute("role", "status");
    const markBar = el("div", "viewer-marks");
    markBar.hidden = true;
    const markText = el("p", "viewer-mark-text");
    const prev = el("button", "viewer-step", "Previous");
    const next = el("button", "viewer-step", "Next");
    prev.type = next.type = "button";
    prev.setAttribute("aria-label", "Previous marked line");
    next.setAttribute("aria-label", "Next marked line");
    const steps = el("div", "viewer-steps");
    const stepCount = el("span", "viewer-step-count");
    steps.append(prev, stepCount, next);
    markBar.append(markText, steps);
    const scroller = el("div", "code-scroll");
    scroller.tabIndex = 0;
    scroller.setAttribute("role", "region");
    const sizer = el("div", "code-sizer");
    const rows = el("div", "code-rows");
    sizer.append(rows);
    scroller.append(sizer);
    const message = el("div", "viewer-message");
    message.hidden = true;
    pane.setAttribute("aria-labelledby", "viewer-title");
    pane.replaceChildren(head, facts, stale, markBar, scroller, message);

    function clamp(value) { return Math.min(0.85, Math.max(0.15, value)); }
    function applySplit() {
      inspector.style.setProperty("--split-top", `${split}fr`);
      inspector.style.setProperty("--split-bottom", `${1 - split}fr`);
      splitter.setAttribute("aria-valuenow", String(Math.round(split * 100)));
    }
    function show(open) {
      pane.hidden = !open;
      splitter.hidden = !open;
      inspector.classList.toggle("split", open);
      document.body.classList.toggle("viewer-open", open);
      if (open) applySplit();
      hooks.layout();
    }

    // ---------------------------------------------------------- rows
    function renderRows(force) {
      if (!file) return;
      const count = file.lines.length;
      const top = scroller.scrollTop, height = scroller.clientHeight || 400;
      const first = Math.max(0, Math.floor(top / lineHeight) - OVERSCAN);
      const last = Math.min(count, Math.ceil((top + height) / lineHeight) + OVERSCAN);
      // Still covered with a margin: nothing to do.
      if (!force && first >= rendered.first && last <= rendered.last) return;
      const from = Math.max(0, first - OVERSCAN / 2), to = Math.min(count, last + OVERSCAN / 2);
      const marked = new Set(marks.lines);
      const current = marks.at >= 0 ? marks.lines[marks.at] : -1;
      const out = [];
      for (let i = from; i < to; i++) {
        const number = i + 1;
        const row = el("div", `code-row${marked.has(number) ? ` mark ${marks.exact ? "exact" : "guess"}` : ""}${number === current ? " current" : ""}`);
        row.dataset.line = String(number);
        row.append(el("span", "code-ln", number), codeText(i));
        out.push(row);
      }
      rows.replaceChildren(...out);
      rows.style.transform = `translateY(${from * lineHeight}px)`;
      rendered = { first: from, last: to };
    }
    function codeText(index) {
      const line = file.lines[index];
      const text = el("span", "code-text");
      if (!file.lang || line.length > PLAIN_LINE) { text.textContent = line; return text; }
      const spans = [];
      scanLine(line, file.states[index], file.lang, spans);
      let at = 0;
      for (const [start, end, kind] of spans) {
        if (start > at) text.append(line.slice(at, start));
        text.append(el("span", `t-${kind === "raw" ? "string" : kind}`, line.slice(start, end)));
        at = end;
      }
      if (at < line.length) text.append(line.slice(at));
      return text;
    }
    scroller.addEventListener("scroll", () => {
      if (frame) return;
      frame = requestAnimationFrame(() => { frame = 0; renderRows(false); });
    }, { passive: true });

    // ---------------------------------------------------------- marks
    function applyMarks(request) {
      marks = { lines: [], exact: false, note: "", at: -1 };
      if (request.lines && request.lines.length) {
        const valid = [...new Set(request.lines)].filter((line) => line >= 1 && line <= file.lines.length).sort((a, b) => a - b);
        marks = { lines: valid, exact: true, note: request.note || "", at: valid.length ? 0 : -1 };
        if (!valid.length) marks.note = `${request.note || "The evidence"} points at line ${request.lines[0]}, past the end of the file; it changed since the index was built.`;
      } else if (request.refersTo) {
        const found = referenceLines(file.lines, request.refersTo);
        const name = found.terms.paths[found.terms.paths.length - 1];
        marks = { lines: found.lines, exact: false, at: found.lines.length ? 0 : -1,
          note: found.how === "import" ? `Lines that import ${name}. GitNexus gives the files, not the line; these were found in the text.`
            : found.how === "name" ? `No import of ${name} found; lines that name ${found.terms.names.join(" or ")}. Found in the text, so some may be unrelated.`
              : `GitNexus reports this ${request.kind || "dependency"} without a line, and no line here names ${found.terms.name}. It may go through a symbol defined elsewhere.` };
      }
      markBar.hidden = !marks.note;
      markBar.classList.toggle("guess", !marks.exact);
      markText.textContent = marks.note;
      steps.hidden = marks.lines.length < 2;
      updateStep();
    }
    function updateStep() {
      stepCount.textContent = marks.lines.length ? `Line ${marks.lines[marks.at]}, ${marks.at + 1} of ${marks.lines.length}` : "";
    }
    function goToMark(index, smooth) {
      if (!marks.lines.length) return;
      marks.at = (index + marks.lines.length) % marks.lines.length;
      updateStep();
      scrollToLine(marks.lines[marks.at], smooth);
      renderRows(true);
    }
    function scrollToLine(line, smooth) {
      const height = scroller.clientHeight || 400;
      const top = Math.max(0, (line - 1) * lineHeight - height / 3);
      scroller.scrollTo({ top, behavior: smooth ? "smooth" : "auto" });
    }
    prev.addEventListener("click", () => goToMark(marks.at - 1, true));
    next.addEventListener("click", () => goToMark(marks.at + 1, true));

    // ---------------------------------------------------------- loading
    function showFacts(payload) {
      facts.replaceChildren(el("span", "viewer-fact", `${payload.line_count.toLocaleString("en")} line${payload.line_count === 1 ? "" : "s"}`),
        el("span", "viewer-fact", payload.bytes >= 1024 ? `${(payload.bytes / 1024).toFixed(1)} KiB` : `${payload.bytes} bytes`),
        el("span", "viewer-fact viewer-node", payload.node));
    }
    function showPath(path) {
      const slash = path.lastIndexOf("/");
      dir.textContent = slash < 0 ? "" : path.slice(0, slash + 1);
      base.textContent = slash < 0 ? path : path.slice(slash + 1);
      title.title = path;
      scroller.setAttribute("aria-label", `Source of ${path}`);
    }
    function refuse(path, text) {
      file = null;
      showPath(path);
      facts.replaceChildren();
      markBar.hidden = true;
      stale.hidden = true;
      scroller.hidden = true;
      message.hidden = false;
      message.replaceChildren(el("p", "viewer-refusal", `Cannot show this file: ${text}`));
      pane.dataset.state = "refused";
    }
    function setText(payload, request, keepScroll) {
      const lines = payload.text.split("\n");
      if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
      for (let i = 0; i < lines.length; i++) if (lines[i].endsWith("\r")) lines[i] = lines[i].slice(0, -1);
      const lang = languageOf(payload.path);
      const started = performance.now();
      file = { path: payload.path, lines, lang, states: lang ? lineStates(lines, lang) : null, hash: payload.hash, node: payload.node, request };
      let columns = 0;
      for (const line of lines) {
        let width = line.length;
        for (let i = 0; i < line.length; i++) if (line[i] === "\t") width += 3;
        if (width > columns) columns = width;
      }
      file.prepareMs = performance.now() - started;
      showPath(payload.path);
      showFacts(payload);
      stale.hidden = true;
      delete pane.dataset.stale;
      message.hidden = true;
      scroller.hidden = false;
      pane.dataset.state = "shown";
      pane.dataset.path = payload.path;
      lineHeight = parseFloat(getComputedStyle(scroller).getPropertyValue("--code-line")) || 20;
      const digits = String(lines.length).length;
      sizer.style.setProperty("--gutter", `${Math.max(3, digits) + 2}ch`);
      sizer.style.setProperty("--cols", String(columns));
      sizer.style.height = `${lines.length * lineHeight}px`;
      applyMarks(request);
      const top = scroller.scrollTop;
      rendered = { first: -1, last: -1 };
      if (keepScroll) { scroller.scrollTop = top; renderRows(true); }
      else if (marks.lines.length) goToMark(0, false);
      else { scroller.scrollTop = 0; renderRows(true); }
    }
    // request: { path, lines?: [n], note?, refersTo?: path, kind? }
    async function open(request) {
      const mine = ++token;
      show(true);
      const problem = unsafePath(request.path);
      if (problem) { refuse(String(request.path || ""), `${problem}.`); return false; }
      if (file && file.path === request.path) {
        // The same file: only the marks change.
        file.request = request;
        applyMarks(request);
        if (marks.lines.length) goToMark(0, true); else renderRows(true);
        return true;
      }
      pane.dataset.state = "loading";
      showPath(request.path);
      try {
        const payload = await hooks.fetch(`/api/source?path=${encodeURIComponent(request.path)}`);
        if (mine !== token) return false;
        setText(payload, request, false);
        return true;
      } catch (error) {
        if (mine === token) refuse(request.path, error.message || String(error));
        return false;
      }
    }
    // The server published a new revision (`reloaded`), or nothing changed
    // there but the file may have changed on disk: reload the text in the
    // first case, only ask for its hash in the second.
    async function sync(reloaded) {
      if (pane.hidden || !file) return;
      const mine = token, current = file;
      try {
        const payload = await hooks.fetch(`/api/source?path=${encodeURIComponent(current.path)}&if_hash=${current.hash}`);
        if (mine !== token || file !== current || payload.unchanged) { if (reloaded) stale.hidden = true; return; }
        if (reloaded) { setText(payload, current.request, true); showStale("Updated with the reloaded architecture: the file changed.", false, false); }
        else showStale("This file changed on disk since it was opened.", true, true);
      } catch (error) {
        if (mine === token && file === current) showStale(`No longer shown by the server: ${error.message || error}`, false, true);
      }
    }
    // A note over the code: stale (the text shown is out of date) or only
    // informative (it was just updated).
    function showStale(text, offerReload, isStale) {
      stale.replaceChildren(el("span", null, text));
      stale.classList.toggle("info", !isStale);
      if (offerReload) {
        const reload = el("button", "text-button", "Reload");
        reload.type = "button";
        reload.addEventListener("click", async () => {
          const current = file;
          if (!current) return;
          try {
            const payload = await hooks.fetch(`/api/source?path=${encodeURIComponent(current.path)}`);
            if (file === current) setText(payload, current.request, true);
          } catch (error) { showStale(`No longer shown by the server: ${error.message || error}`, false, true); }
        });
        stale.append(reload);
      }
      stale.hidden = false;
      if (isStale) pane.dataset.stale = "true"; else delete pane.dataset.stale;
    }
    function close() {
      if (pane.hidden) return;
      token++;
      file = null;
      delete pane.dataset.path;
      delete pane.dataset.stale;
      show(false);
    }
    closeButton.addEventListener("click", close);
    pane.addEventListener("keydown", (event) => {
      if (event.key === "Escape") { event.stopPropagation(); close(); }
    });
    copy.addEventListener("click", async () => {
      const path = title.title;
      let done = false;
      try { await navigator.clipboard.writeText(path); done = true; } catch { /* fall back below */ }
      if (!done) {
        const area = el("textarea", "visually-hidden");
        area.value = path;
        document.body.append(area);
        area.select();
        try { done = document.execCommand("copy"); } catch { done = false; }
        area.remove();
      }
      copy.textContent = done ? "Copied" : "Copy failed";
      setTimeout(() => { copy.textContent = "Copy path"; }, 1500);
    });

    // ---------------------------------------------------------- divider
    splitter.tabIndex = 0;
    splitter.setAttribute("aria-label", "Resize the details and the source");
    splitter.setAttribute("aria-valuemin", "15");
    splitter.setAttribute("aria-valuemax", "85");
    splitter.setAttribute("aria-controls", "pane-details pane-secondary");
    const setSplit = (value) => { split = clamp(value); applySplit(); save({ ...stored(), split }); };
    splitter.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      splitter.setPointerCapture(event.pointerId);
      splitter.classList.add("dragging");
      const box = inspector.getBoundingClientRect();
      const move = (moveEvent) => { split = clamp((moveEvent.clientY - box.top) / box.height); applySplit(); };
      const up = () => {
        splitter.classList.remove("dragging");
        splitter.removeEventListener("pointermove", move);
        splitter.removeEventListener("pointerup", up);
        splitter.removeEventListener("pointercancel", up);
        setSplit(split);
        renderRows(true);
      };
      splitter.addEventListener("pointermove", move);
      splitter.addEventListener("pointerup", up);
      splitter.addEventListener("pointercancel", up);
    });
    splitter.addEventListener("keydown", (event) => {
      if (event.key === "ArrowUp") setSplit(split - 0.05);
      else if (event.key === "ArrowDown") setSplit(split + 0.05);
      else if (event.key === "Home") setSplit(0.15);
      else if (event.key === "End") setSplit(0.85);
      else return;
      event.preventDefault();
      renderRows(true);
    });
    splitter.addEventListener("dblclick", () => { setSplit(0.4); renderRows(true); });
    window.addEventListener("resize", () => renderRows(true));

    return {
      open, sync, close,
      isOpen: () => !pane.hidden,
      current: () => (file ? { path: file.path, lines: file.lines.length, marks: [...marks.lines], exact: marks.exact, prepareMs: file.prepareMs, rendered: rows.childElementCount } : null),
    };
  }

  return { create, unsafePath, referenceLines, scanLine, languageOf };
})();
