import { useEffect, type RefObject } from "react";

/** Open `<details data-popover>` menus that `target` is not inside. */
function popoversOutside(target: Node | null): HTMLDetailsElement[] {
  return Array.from(document.querySelectorAll<HTMLDetailsElement>("details[data-popover][open]"))
    .filter(popover => !(target && popover.contains(target)));
}

/**
 * Closes `<details data-popover>` menus (model pill, composer menus, project switcher) on a
 * click anywhere outside them or on Escape, so they never need a second click to close.
 */
export function usePopoverDismissal(): void {
  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      for (const popover of popoversOutside(event.target as Node | null)) popover.removeAttribute("open");
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      for (const popover of popoversOutside(null)) popover.removeAttribute("open");
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, []);
}

/** Calls `close` on a click outside `ref` or on Escape while `open`. */
export function useDismiss(ref: RefObject<HTMLElement | null>, open: boolean, close: () => void): void {
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) close();
    };
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape") close(); };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [ref, open, close]);
}
