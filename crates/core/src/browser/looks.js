// What a page's looks check measures in one colour scheme (run by the `check_looks` action).
// Returns {report, elements}: `report` is plain data whose items point into `elements` by
// index, so the caller can name each element with the snapshot's refs. The work is bounded:
// at most ELEMENTS elements and TEXTS text nodes are looked at, ITEMS items kept per kind.
(async () => {
  const ELEMENTS = 5000;
  const TEXTS = 5000;
  const ITEMS = 15;
  const SHEETS = 20;
  const SHEET_BYTES = 300000;
  const RULES = 5000;

  const elements = [];
  const indexOf = new Map();
  const ref = (element) => {
    if (!element || element === document.body || element === document.documentElement) return -1;
    if (!indexOf.has(element)) {
      indexOf.set(element, elements.length);
      elements.push(element);
    }
    return indexOf.get(element);
  };
  const items = { contrast: [], control: [], overflow: [], clipped: [], code: [] };
  const counts = { contrast: 0, control: 0, overflow: 0, clipped: 0, code: 0 };
  const add = (kind, text, element) => {
    counts[kind] += 1;
    if (items[kind].length < ITEMS) items[kind].push({ text, describe: describe(element), el: ref(element) });
  };

  // Settle any colour transition the scheme switch started, so colours are the final ones.
  const settled = Date.now() + 600;
  while (Date.now() < settled
    && document.getAnimations().some((a) => a.playState === 'running' && typeof CSSTransition !== 'undefined' && a instanceof CSSTransition)) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }

  const clip = (text, limit) => {
    const flat = String(text || '').replace(/\s+/g, ' ').trim();
    return flat.length > limit ? flat.slice(0, limit - 1) + '…' : flat;
  };
  const ROLE = { A: 'link', BUTTON: 'button', SELECT: 'combobox', TEXTAREA: 'textbox', IMG: 'img', NAV: 'navigation', LI: 'listitem', LABEL: 'label', H1: 'heading', H2: 'heading', H3: 'heading', H4: 'heading', H5: 'heading', H6: 'heading' };
  function role(element) {
    const explicit = element.getAttribute('role');
    if (explicit) return explicit.split(/\s+/)[0];
    if (element.tagName === 'INPUT') {
      const type = (element.getAttribute('type') || 'text').toLowerCase();
      if (['button', 'submit', 'reset'].includes(type)) return 'button';
      if (['checkbox', 'radio'].includes(type)) return type;
      return 'textbox';
    }
    if (element.tagName === 'A' && !element.hasAttribute('href')) return 'a';
    const known = ROLE[element.tagName];
    if (known) return known;
    const name = element.tagName.toLowerCase();
    const first = typeof element.className === 'string' ? element.className.trim().split(/\s+/)[0] : '';
    return first ? `${name}.${first}` : name;
  }
  // A field's value never leaves this script, whatever the field: only whether it is filled
  // (passwords, tokens and card numbers live in fields the page may not mark as secret).
  const filled = (field) => field.tagName !== 'SELECT' && String(field.value || '').length > 0;
  function describe(element) {
    if (!element) return 'the page';
    if (element === document.body) return 'body';
    if (element === document.documentElement) return 'html';
    const field = element.matches('input, textarea, select');
    const label = field && element.labels && element.labels[0] ? element.labels[0].innerText : '';
    const name = element.getAttribute('aria-label') || (field ? label : clip(element.innerText, 50))
      || element.getAttribute('placeholder') || element.getAttribute('title')
      || element.getAttribute('alt') || '';
    const shown = clip(name, 50);
    const described = shown ? `${role(element)} "${shown}"` : role(element);
    return field && filled(element) ? `${described} (filled)` : described;
  }

  // Colours.
  function parse(color) {
    const m = /rgba?\(([^)]+)\)/.exec(color || '');
    if (!m) return null;
    const parts = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
    if (parts.length < 3 || parts.some((p) => Number.isNaN(p))) return null;
    return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 };
  }
  const over = (top, bottom) => ({
    r: top.r * top.a + bottom.r * (1 - top.a),
    g: top.g * top.a + bottom.g * (1 - top.a),
    b: top.b * top.a + bottom.b * (1 - top.a),
    a: 1,
  });
  const channel = (v) => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; };
  const luminance = (c) => 0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b);
  const ratio = (a, b) => {
    const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
    return (x + 0.05) / (y + 0.05);
  };
  const hex = (c) => '#' + [c.r, c.g, c.b].map((v) => Math.round(v).toString(16).padStart(2, '0')).join('');
  const darkScheme = window.matchMedia('(prefers-color-scheme: dark)').matches;
  const rootScheme = getComputedStyle(document.documentElement).colorScheme || '';
  const canvasDark = rootScheme.includes('dark') && (!rootScheme.includes('light') || darkScheme);
  const canvas = canvasDark ? { r: 18, g: 18, b: 18, a: 1 } : { r: 255, g: 255, b: 255, a: 1 };

  const styles = new Map();
  const style = (element) => {
    let found = styles.get(element);
    if (!found) { found = getComputedStyle(element); styles.set(element, found); }
    return found;
  };
  // The colour behind `element` (its own background included), or null when an image or
  // gradient is in the way and the colour cannot be known.
  const backgrounds = new Map();
  function background(element) {
    if (!element) return canvas;
    if (backgrounds.has(element)) return backgrounds.get(element);
    const s = style(element);
    let result;
    if (s.backgroundImage && s.backgroundImage !== 'none') {
      result = null;
    } else {
      const own = parse(s.backgroundColor);
      const parent = element.parentElement;
      if (own && own.a >= 1) result = own;
      else {
        const below = background(parent);
        result = below && own && own.a > 0 ? over(own, below) : below;
      }
    }
    backgrounds.set(element, result);
    return result;
  }

  function visible(element) {
    const s = style(element);
    if (s.visibility !== 'visible' || s.display === 'none') return false;
    if (parseFloat(s.opacity) === 0) return false;
    const rect = element.getBoundingClientRect();
    if (rect.width <= 2 || rect.height <= 2) return false;
    if (s.clip && s.clip !== 'auto') return false;
    if (s.clipPath && s.clipPath !== 'none') return false;
    return true;
  }
  const directText = (element) => {
    for (const node of element.childNodes) {
      if (node.nodeType === Node.TEXT_NODE && node.data.trim()) return true;
    }
    return false;
  };
  const CONTROL = 'button, a[href], select, textarea, [role="button"], input:not([type="hidden"]):not([type="checkbox"]):not([type="radio"]):not([type="range"]):not([type="color"]):not([type="file"]):not([type="image"])';
  const SKIP = 'script, style, noscript, template, svg, canvas, video, iframe, object';
  const opacityOf = (element) => {
    let value = 1;
    for (let e = element; e && value > 0; e = e.parentElement) value *= parseFloat(style(e).opacity) || 0;
    return value;
  };

  // Every element once, in document order.
  const all = document.body ? document.body.querySelectorAll('*') : [];
  const checked = Math.min(all.length, ELEMENTS);
  // Controls with readable text of their own or in a child.
  const withText = new Set();
  const readable = (element) => {
    const host = element.closest(CONTROL);
    if (host) withText.add(host);
  };
  const controls = [];
  const viewWidth = document.documentElement.clientWidth;
  // Elements already listed as sticking out; their children are not listed again.
  const outermost = new Set();
  const insideListed = (element) => {
    for (let e = element.parentElement; e; e = e.parentElement) if (outermost.has(e)) return true;
    return false;
  };
  const scrollsSideways = (element) => {
    for (let e = element.parentElement; e && e !== document.body && e !== document.documentElement; e = e.parentElement) {
      const x = style(e).overflowX;
      if (x !== 'visible') return true;
    }
    return false;
  };
  for (let i = 0; i < checked; i += 1) {
    const element = all[i];
    if (element.closest(SKIP)) continue;
    if (!visible(element)) continue;
    if (opacityOf(element) < 0.1) continue;
    const s = style(element);
    const isControl = element.matches(CONTROL);
    if (isControl) controls.push(element);

    // Text against what is behind it.
    const isField = element.matches('input, textarea, select');
    const hasText = isField
      ? filled(element) || Boolean((element.getAttribute('placeholder') || '').trim())
      : directText(element);
    if (hasText && (!s.textShadow || s.textShadow === 'none') && parseFloat(s.webkitTextStrokeWidth || '0') === 0) {
      const placeholderOnly = isField && !filled(element);
      const ink = parse(placeholderOnly ? getComputedStyle(element, '::placeholder').color : s.color);
      const behind = background(element);
      if (ink && behind) {
        const shown = over(ink, behind);
        const value = ratio(shown, behind);
        if (value < 1.5) {
          add('contrast', `invisible text, contrast ${value.toFixed(1)}:1 (${hex(shown)} on ${hex(behind)})`, element);
        } else if (isControl && value < 3) {
          add('contrast', `faint control text, contrast ${value.toFixed(1)}:1 (${hex(shown)} on ${hex(behind)})`, element);
          readable(element);
        } else {
          readable(element);
        }
      } else {
        readable(element);
      }
    }

    // Off the side of the page.
    const rect = element.getBoundingClientRect();
    const sticksOut = rect.right > viewWidth + 1 && rect.left > -1;
    if (sticksOut && s.position !== 'fixed' && !scrollsSideways(element)
      && !insideListed(element)) {
      outermost.add(element);
      const how = rect.left >= viewWidth ? 'off the page to the right' : `cut off by the right edge of the page (${Math.round(rect.right - viewWidth)}px past it)`;
      add('overflow', how, element);
    }

    // Its own text cut off by its box.
    if (directText(element) && ['hidden', 'clip'].some((v) => s.overflowX === v || s.overflowY === v)
      && s.textOverflow !== 'ellipsis' && (!s.webkitLineClamp || s.webkitLineClamp === 'none')) {
      const wide = element.scrollWidth > element.clientWidth + 2 && s.overflowX !== 'visible';
      const tall = element.scrollHeight > element.clientHeight + 2 && s.overflowY !== 'visible';
      if (element.clientWidth > 2 && element.clientHeight > 2 && (wide || tall)) {
        const size = wide ? `${element.scrollWidth}px of text in a ${element.clientWidth}px wide box` : `${element.scrollHeight}px of text in a ${element.clientHeight}px tall box`;
        add('clipped', `text cut off: ${size}`, element);
      }
    }
  }

  // Buttons and fields a person cannot see: no readable text or picture, and no edge.
  for (const element of controls) {
    let content = withText.has(element);
    if (!content) {
      for (const media of element.querySelectorAll('img, svg, canvas, video, picture')) {
        const r = media.getBoundingClientRect();
        if (r.width > 2 && r.height > 2) { content = true; break; }
      }
    }
    const s = style(element);
    const behind = background(element.parentElement);
    const own = background(element);
    const edgeColour = parse(s.borderTopColor);
    const border = parseFloat(s.borderTopWidth) > 0 && s.borderTopStyle !== 'none' && s.borderTopStyle !== 'hidden'
      && edgeColour && edgeColour.a > 0 && (!behind || ratio(over(edgeColour, behind), behind) >= 1.2);
    const filled = !own || !behind || ratio(own, behind) >= 1.2;
    const shadow = s.boxShadow && s.boxShadow !== 'none';
    const field = element.matches('input, textarea, select');
    if (field ? !border && !filled && !shadow : !content && !border && !filled && !shadow) {
      add('control', field ? 'field with no visible edge' : 'control with no visible text or edge', element);
    }
  }

  // Source code shown as page text.
  const CODE = /^(\/\/|\/\*|<!--|<\/?[a-z][a-z0-9-]*[\s>]|\{|\}|function[\s(]|import\s.+\sfrom\s|export\s+(default|const|function)|const\s+\w+\s*=|let\s+\w+\s*=|var\s+\w+\s*=|\$\{)/i;
  const CODE_CONTAINER = 'pre, code, kbd, samp, textarea, script, style, noscript, template, [contenteditable=""], [contenteditable="true"]';
  if (document.body) {
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    let seen = 0;
    for (let node = walker.nextNode(); node && seen < TEXTS; node = walker.nextNode()) {
      seen += 1;
      const first = node.data.trim().split('\n')[0].trim();
      if (!first || !CODE.test(first)) continue;
      const parent = node.parentElement;
      if (!parent || parent.closest(CODE_CONTAINER)) continue;
      const s = style(parent);
      if (s.visibility !== 'visible' || s.display === 'none') continue;
      const range = document.createRange();
      range.selectNodeContents(node);
      const box = range.getBoundingClientRect();
      if (box.width <= 1 || box.height <= 1) continue;
      add('code', `text that looks like source code: "${clip(first, 80)}"`, parent);
    }
  }

  // Whole-page problems.
  const page = [];
  if (document.compatMode === 'BackCompat') {
    page.push('the page renders in quirks mode: <!doctype html> is not the very first thing in the HTML, so layout and sizes follow old browser rules');
  }
  const scrollWidth = document.documentElement.scrollWidth;
  if (scrollWidth > viewWidth + 1) {
    page.push(`the page scrolls sideways: it is ${scrollWidth}px wide in a ${viewWidth}px wide window`);
  }

  // CSS rules the browser dropped as invalid (it says nothing about them anywhere else).
  const dropped = [];
  let droppedCount = 0;
  const VENDOR = /(^|[^a-z])-(moz|ms|o)-/i;
  async function sourceOf(sheet) {
    const owner = sheet.ownerNode;
    if (owner && owner.tagName === 'STYLE') return owner.textContent || '';
    if (!sheet.href) return null;
    try {
      if (new URL(sheet.href).origin !== location.origin) return null;
      const response = await fetch(sheet.href, { cache: 'force-cache', credentials: 'same-origin' });
      if (!response.ok) return null;
      return (await response.text()).slice(0, SHEET_BYTES);
    } catch (_) {
      return null;
    }
  }
  function preludes(source) {
    const found = [];
    let depth = 0;
    let start = 0;
    for (let i = 0; i < source.length && found.length < RULES; i += 1) {
      const c = source[i];
      if (c === '/' && source[i + 1] === '*') {
        const end = source.indexOf('*/', i + 2);
        i = end < 0 ? source.length : end + 1;
      } else if (c === '"' || c === "'") {
        for (i += 1; i < source.length && source[i] !== c && source[i] !== '\n'; i += 1) {
          if (source[i] === '\\') i += 1;
        }
      } else if (c === '{') {
        if (depth === 0) found.push({ text: source.slice(start, i), at: start });
        depth += 1;
      } else if (c === '}') {
        depth = Math.max(0, depth - 1);
        if (depth === 0) start = i + 1;
      } else if (c === ';' && depth === 0) {
        start = i + 1;
      }
    }
    return found;
  }
  const sheets = Array.from(document.styleSheets).slice(0, SHEETS);
  for (const sheet of sheets) {
    const source = await sourceOf(sheet);
    if (source === null) continue;
    const name = sheet.href ? sheet.href.split(/[?#]/)[0].split('/').pop() || sheet.href : 'a <style> block';
    for (const prelude of preludes(source)) {
      const raw = prelude.text;
      const selector = raw.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/<!--|-->/g, ' ').trim();
      if (!selector || selector.startsWith('@') || VENDOR.test(selector)) continue;
      let valid = true;
      try { document.createDocumentFragment().querySelector(selector); } catch (_) { valid = false; }
      if (valid) continue;
      droppedCount += 1;
      if (dropped.length >= 5) continue;
      const offset = raw.search(/\S/);
      const line = source.slice(0, prelude.at + Math.max(0, offset)).split('\n').length;
      const firstLine = clip(selector.split('\n')[0], 80);
      const why = selector.includes('//') ? ' (CSS has no // comments; only /* */)' : '';
      dropped.push(`${name} line ${line}: the browser drops the whole rule starting "${firstLine}" as invalid${why}`);
    }
  }
  if (droppedCount > dropped.length) dropped.push(`and ${droppedCount - dropped.length} more dropped rules`);

  return {
    report: {
      scheme: darkScheme ? 'dark' : 'light',
      checked,
      capped: all.length > ELEMENTS,
      page,
      dropped,
      items,
      counts,
    },
    elements,
  };
})()
