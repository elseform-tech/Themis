// The page's structure for the snapshot action's `layout`: visible containers, where they are and a
// selector for each. The accessibility outline leaves plain containers out, so without this an
// agent can only guess where a column is. Text is page data; a field's value is never read.
(function () {
  const MAX_LINES = 200;
  const MAX_VISITED = 20000;
  const SKIP = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "BR", "HR", "LINK", "META"]);
  const INLINE = new Set(["SPAN", "B", "I", "EM", "STRONG", "SMALL", "CODE", "SVG", "PATH"]);
  const NAMED = new Set(["BUTTON", "A", "INPUT", "SELECT", "TEXTAREA", "CANVAS", "IMG", "VIDEO", "IFRAME", "H1", "H2", "H3", "H4", "LABEL"]);
  const lines = [];
  let visited = 0;
  let omitted = 0;

  const shown = (e, r) => {
    const s = getComputedStyle(e);
    return s.display !== "none" && s.visibility !== "hidden" && (r.width > 0 && r.height > 0 || e.tagName === "CANVAS");
  };
  const selector = (e) => {
    if (e.id && document.querySelectorAll("#" + CSS.escape(e.id)).length === 1) return "#" + CSS.escape(e.id);
    const parts = [];
    for (let n = e; n && n !== document.body; n = n.parentElement) {
      const c = typeof n.className === "string" && n.className.trim() ? "." + n.className.trim().split(/\s+/).map(CSS.escape).join(".") : "";
      const index = n.parentElement ? [...n.parentElement.children].indexOf(n) + 1 : 1;
      parts.unshift(n.tagName.toLowerCase() + c + ":nth-child(" + index + ")");
      const found = parts.join(" > ");
      if (document.querySelectorAll(found).length === 1) return found;
    }
    return parts.join(" > ");
  };
  const canvas = (e) => {
    if (!e.width || !e.height) return "nothing drawn";
    try {
      const d = e.getContext("2d");
      if (!d) return "not a 2d canvas, can't read";
      const w = Math.min(e.width, 400), h = Math.min(e.height, 400);
      const p = d.getImageData(0, 0, w, h).data;
      for (let i = 4; i < p.length; i += 4) {
        if (p[i] !== p[0] || p[i + 1] !== p[1] || p[i + 2] !== p[2] || p[i + 3] !== p[3]) return "has drawing";
      }
      return "blank";
    } catch (_) {
      return "can't read";
    }
  };
  const own = (e) => [...e.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join(" ");
  const label = (e) => {
    if (e.tagName === "INPUT" || e.tagName === "TEXTAREA" || e.tagName === "SELECT") return e.placeholder || e.getAttribute("aria-label") || "";
    const leaf = e.children.length === 0 || [...e.children].every((c) => INLINE.has(c.tagName.toUpperCase()));
    const named = e.querySelector && e.querySelector(":scope > h1, :scope > h2, :scope > h3, :scope > h4, :scope > header");
    return (leaf ? e.innerText : named ? named.innerText : own(e)) || "";
  };
  const worth = (e) => NAMED.has(e.tagName) || e.draggable || e.id || (typeof e.className === "string" && e.className.trim()) || e.getAttribute("role") || own(e).trim();

  const walk = (e, depth) => {
    for (const child of e.children) {
      if (++visited > MAX_VISITED) return;
      if (SKIP.has(child.tagName) || INLINE.has(child.tagName.toUpperCase())) continue;
      const r = child.getBoundingClientRect();
      if (!shown(child, r)) continue;
      let next = depth;
      if (worth(child)) {
        if (lines.length >= MAX_LINES) { omitted++; }
        else {
          const c = typeof child.className === "string" && child.className.trim() ? "." + child.className.trim().split(/\s+/)[0] : "";
          const id = child.id ? "#" + child.id : "";
          const text = label(child).trim().replace(/\s+/g, " ").slice(0, 40).replace(/"/g, "'");
          const off = r.bottom < 0 || r.top > innerHeight || r.right < 0 || r.left > innerWidth;
          const size = child.tagName === "CANVAS" ? child.width + "×" + child.height : Math.round(r.width) + "×" + Math.round(r.height);
          lines.push("  ".repeat(depth) + child.tagName.toLowerCase() + id + c + (text ? ' "' + text + '"' : "") +
            " (" + Math.round(r.left) + "," + Math.round(r.top) + " " + size + ")" +
            (child.draggable ? " (draggable)" : "") + (off ? " (off-screen)" : "") +
            (child.tagName === "CANVAS" ? " (" + canvas(child) + ")" : "") + " → " + selector(child));
        }
        next = depth + 1;
      }
      walk(child, next);
    }
  };
  walk(document.body, 0);
  if (omitted || visited > MAX_VISITED) lines.push((omitted || "some") + " more not shown; ask for one part with a selector in the devtools action");
  return "visible page " + innerWidth + "×" + innerHeight + ", positions are (left,top width×height)\n" + lines.join("\n");
})()
