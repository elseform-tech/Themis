import { useLayoutEffect, type RefObject } from "react";

/** `field-sizing: content` sizes a select to its chosen option (Chromium, WebView2). */
function fieldSizing(): boolean {
  return typeof CSS !== "undefined" && typeof CSS.supports === "function" && CSS.supports("field-sizing", "content");
}

/** Sizes a select to its chosen option where `field-sizing: content` is missing (older macOS WebKit, WebKitGTK), so
 *  its chevron sits right after the label instead of after the longest option. Measures the label in the select's
 *  own font with a hidden probe. */
export function useFitSelect(ref: RefObject<HTMLSelectElement | null>): void {
  useLayoutEffect(() => {
    const select = ref.current;
    if (!select || fieldSizing()) return;
    // Every render, since a select can mount late (the Author filter); measured only when its label changes.
    const label = select.selectedOptions[0]?.textContent ?? "";
    if (select.dataset.fittedTo === label) return;
    select.dataset.fittedTo = label;
    const style = getComputedStyle(select);
    const probe = document.createElement("span");
    probe.style.cssText = "position: absolute; visibility: hidden; white-space: pre;";
    probe.style.font = style.font;
    probe.style.fontFamily = style.fontFamily;
    probe.style.fontSize = style.fontSize;
    probe.style.fontWeight = style.fontWeight;
    probe.style.letterSpacing = style.letterSpacing;
    probe.textContent = label;
    document.body.append(probe);
    const text = probe.getBoundingClientRect().width;
    probe.remove();
    const padding = (parseFloat(style.paddingLeft) || 0) + (parseFloat(style.paddingRight) || 0);
    // Two pixels for rounding, so the label never ellipsizes at its own width.
    select.style.width = `${Math.ceil(text + padding) + 2}px`;
  });
}
