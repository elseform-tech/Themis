// Loads the page's app.js against a small DOM built from index.html, then adds two errands,
// ticks the first and presses "Clear completed". Exit 0 when the list and count are right.
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const root = process.argv[2];
const html = fs.readFileSync(path.join(root, "index.html"), "utf8");

class Element {
  constructor(tag, id) {
    this.tagName = tag.toUpperCase();
    this.id = id || "";
    this.children = [];
    this.listeners = {};
    this.value = "";
    this.checked = false;
    this.type = "";
    this._text = "";
  }
  appendChild(child) { this.children.push(child); return child; }
  append(...nodes) { nodes.forEach((n) => this.appendChild(typeof n === "string" ? text(n) : n)); }
  addEventListener(type, fn) { (this.listeners[type] = this.listeners[type] || []).push(fn); }
  dispatch(type) { (this.listeners[type] || []).forEach((fn) => fn({ target: this, type, preventDefault() {} })); }
  click() { this.dispatch("click"); }
  set innerHTML(value) { if (value === "") this.children = []; else this._text = value; }
  get innerHTML() { return this._text; }
  set textContent(value) { this._text = String(value); this.children = []; }
  get textContent() { return this._text + this.children.map((c) => c.textContent).join(""); }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter((c) => c !== this); }
}
function text(value) { const node = new Element("#text"); node._text = value; return node; }

const byId = {};
const buttons = [];
for (const match of html.matchAll(/<(\w+)([^>]*)>([^<]*)/g)) {
  const id = /\bid="([^"]+)"/.exec(match[2]);
  if (!id) continue;
  const element = new Element(match[1], id[1]);
  byId[id[1]] = element;
  if (match[1] === "button") buttons.push([match[3].trim(), element]);
}
const document = {
  getElementById: (id) => byId[id] || null,
  querySelector: (selector) => (selector.startsWith("#") ? byId[selector.slice(1)] || null : null),
  createElement: (tag) => new Element(tag),
  createTextNode: text,
  addEventListener: () => {},
};
const errors = [];
const sandbox = { document, window: {}, console: { log() {}, warn() {}, error: (...a) => errors.push(a.join(" ")) } };
sandbox.window.document = document;
try {
  vm.runInNewContext(fs.readFileSync(path.join(root, "app.js"), "utf8"), sandbox);
} catch (error) {
  console.log(`app.js threw while loading: ${error.message}`);
  process.exit(1);
}
const input = byId["new-item"];
const add = byId["add"];
const clear = (buttons.find(([label]) => /clear completed/i.test(label)) || [])[1];
const list = byId["list"];
if (!input || !add || !clear || !list) {
  console.log("the page lost one of: #new-item, #add, the Clear completed button, #list");
  process.exit(1);
}
for (const errand of ["milk", "eggs"]) {
  input.value = errand;
  add.click();
}
const first = list.children[0] && list.children[0].children.find((c) => c.tagName === "INPUT");
if (!first) { console.log("no checkbox rendered"); process.exit(1); }
first.checked = true;
first.dispatch("change");
clear.click();
const left = list.children.map((li) => li.textContent.trim());
const count = byId["count"] ? byId["count"].textContent.trim() : "";
if (left.length === 1 && left[0] === "eggs" && /^1 left$/.test(count)) {
  console.log("ok");
  process.exit(0);
}
console.log(`after clearing: list ${JSON.stringify(left)}, count ${JSON.stringify(count)}`);
process.exit(1);
