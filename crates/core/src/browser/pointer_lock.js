// Headless Chromium refuses pointer lock outright, so first-person pages could
// never run there. This stands in for it: the page sees a lock, and mouse moves
// keep their real movementX and movementY. The cursor is not actually confined.
(() => {
  let locked = null;
  const changed = () => document.dispatchEvent(new Event("pointerlockchange"));
  Object.defineProperty(Document.prototype, "pointerLockElement", {
    configurable: true,
    get() {
      return locked;
    },
  });
  Element.prototype.requestPointerLock = function () {
    locked = this;
    changed();
    return Promise.resolve();
  };
  Document.prototype.exitPointerLock = function () {
    locked = null;
    changed();
  };
})();
