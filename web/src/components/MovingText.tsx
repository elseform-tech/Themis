import { useLayoutEffect, useRef } from "react";
import "./MovingText.css";

/** One leftward pass on hover/focus; the accessible name stays complete. */
export function MovingText({ children }: { children: string }) {
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const window = ref.current;
    if (!window) return;
    const measure = () => {
      const distance = Math.max(0, (window.firstElementChild?.scrollWidth ?? 0) - window.clientWidth);
      window.classList.toggle("is-overflowing", distance > 0);
      window.style.setProperty("--title-end", `${-distance}px`);
      window.style.setProperty("--title-duration", `${Math.max(6, distance / 24)}s`);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(window);
    if (window.firstElementChild) observer.observe(window.firstElementChild);
    return () => observer.disconnect();
  }, [children]);
  return <span className="themis-moving-window" ref={ref}><span className="themis-moving-text">{children}</span></span>;
}
