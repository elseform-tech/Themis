import { useLayoutEffect, useRef } from "react";
import "./MovingText.css";

/** Overflowing text makes one leftward pass on hover/focus, or with `loop` scrolls start to end and back; the accessible name stays complete. */
export function MovingText({ children, loop = false }: { children: string; loop?: boolean }) {
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const window = ref.current;
    if (!window) return;
    const measure = () => {
      const distance = Math.max(0, (window.firstElementChild?.scrollWidth ?? 0) - window.clientWidth);
      window.classList.toggle("is-overflowing", distance > 0);
      window.style.setProperty("--title-end", `${-distance}px`);
      window.style.setProperty("--title-duration", `${Math.max(loop ? 8 : 6, distance / (loop ? 12 : 24))}s`);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(window);
    if (window.firstElementChild) observer.observe(window.firstElementChild);
    return () => observer.disconnect();
  }, [children, loop]);
  return <span className={`themis-moving-window${loop ? " themis-moving-window--loop" : ""}`} ref={ref}><span className="themis-moving-text">{children}</span></span>;
}
