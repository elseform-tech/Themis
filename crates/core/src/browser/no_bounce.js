// On a Mac a page follows a scroll past its edge and springs back, and a trackpad can wobble even a
// page with nothing to scroll. In a panel that reads as the page coming loose, so it starts off.
// `:where` has no weight, so a page's own rule wins, and an adopted sheet leaves its DOM alone.
(() => {
  const sheet = new CSSStyleSheet();
  sheet.replaceSync(":where(html, body) { overscroll-behavior: none; }");
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
})();
