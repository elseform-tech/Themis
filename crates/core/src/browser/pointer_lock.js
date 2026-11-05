// The browser runs headless or off screen and can't lock a real pointer, so
// first-person pages could never run there. This stands in for it: the page sees a lock, and mouse moves
// keep their real movementX and movementY. The cursor is not actually confined:
// browser_move's dx and dy carry on past the page's edge, and clicks stay put.
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
    if (!locked) return;
    locked = null;
    changed();
  };
  // Escape gives the mouse back, as a real browser does.
  document.addEventListener(
    "keydown",
    (event) => {
      if (event.key === "Escape" && locked) document.exitPointerLock();
    },
    true,
  );
})();
