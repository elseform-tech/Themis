/** True for targets where typed characters belong to the control, or that sit in the composer. */
export function isTextEntryTarget(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  if (target.closest(".themis-thread-composer")) return true;
  if (target.closest("input, textarea, select, [contenteditable]:not([contenteditable='false']), [role='textbox']")) return true;
  return false;
}

/** A bare printable key (no Cmd/Ctrl/Alt): the kind that must never act as an app-wide shortcut while the user may be typing. */
export function isBareCharacterKey(event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey">): boolean {
  return event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey;
}

/** Single-key shortcuts may fire only outside text entry and composer controls (shield, send, + menu). */
export function singleKeyShortcutAllowed(event: KeyboardEvent): boolean {
  return !event.isComposing && isBareCharacterKey(event) && !isTextEntryTarget(event.target);
}
