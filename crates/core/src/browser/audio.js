// Runs in every document of the chat's browser before page scripts. Chrome has
// no per-tab mute, so while the Workspace speaker is off, media elements are
// muted and Web Audio contexts suspended. Pages still read back the `muted`
// value they set themselves, so players do not fight the switch.
(() => {
  if (window.__themisAudio) return;
  const state = { muted: true, contexts: new Set() };
  const native = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "muted");
  const wanted = new WeakMap();
  const apply = media => native.set.call(media, state.muted || (wanted.has(media) ? wanted.get(media) : media.defaultMuted));
  Object.defineProperty(HTMLMediaElement.prototype, "muted", {
    configurable: true,
    enumerable: native.enumerable,
    get() { return wanted.has(this) ? wanted.get(this) : native.get.call(this); },
    set(value) { wanted.set(this, Boolean(value)); apply(this); },
  });
  // Elements parsed or added after this script, including autoplaying ones.
  new MutationObserver(records => {
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (node instanceof HTMLMediaElement) apply(node);
        else if (node.querySelectorAll) node.querySelectorAll("audio, video").forEach(apply);
      }
    }
  }).observe(document, { childList: true, subtree: true });
  document.addEventListener("play", event => { if (event.target instanceof HTMLMediaElement) apply(event.target); }, true);
  const play = HTMLMediaElement.prototype.play;
  HTMLMediaElement.prototype.play = function (...args) {
    if (state.muted) native.set.call(this, true);
    return play.apply(this, args);
  };
  const Context = window.AudioContext;
  const resume = Context && Context.prototype.resume;
  if (Context) {
    window.AudioContext = class extends Context {
      constructor(...args) {
        super(...args);
        state.contexts.add(this);
        if (state.muted) this.suspend();
      }
    };
    Context.prototype.resume = function () {
      return state.muted ? Promise.resolve() : resume.call(this);
    };
  }
  window.__themisAudio = muted => {
    state.muted = muted;
    document.querySelectorAll("audio, video").forEach(apply);
    for (const context of state.contexts) {
      if (muted) context.suspend(); else resume.call(context);
    }
    for (const frame of Array.from(window.frames)) {
      try { frame.__themisAudio?.(muted); } catch { /* another site's frame */ }
    }
  };
})();
