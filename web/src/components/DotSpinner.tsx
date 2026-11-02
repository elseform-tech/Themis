import "./DotSpinner.css";

/** The 3×3 dot grid status icons are drawn on, in a 24-unit box; cells 0–8 row by row. */
export const DOT_GRID = Array.from({ length: 9 }, (_, cell) => [5 + 7 * (cell % 3), 5 + 7 * Math.floor(cell / 3)] as const);
/** The edge cells clockwise from the top-left; the working light walks round them. */
const SQUARE = [0, 1, 2, 5, 8, 7, 6, 3];

/** Work in progress: the grid's eight edge dots light one after another, so the light walks
 *  round the square in warm gold. Used by the sidebar's working chats and the to-do list's
 *  current item. Decorative: the caller says what is working. */
export function DotSpinner({ className }: { className?: string }) {
  return (
    <svg className={`themis-dot-spinner${className ? ` ${className}` : ""}`} viewBox="0 0 24 24" aria-hidden="true">
      {SQUARE.map((cell, step) => <circle key={cell} className="themis-dot-spinner-dot" cx={DOT_GRID[cell]![0]} cy={DOT_GRID[cell]![1]} r="2.1" style={{ animationDelay: `${(step * 0.15 - 1.2).toFixed(2)}s` }} />)}
    </svg>
  );
}
