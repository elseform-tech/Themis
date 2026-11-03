import { useLayoutEffect, useRef, useState } from "react";
import { DotSpinner } from "../components/DotSpinner";
import type { TodoItem } from "../lib/types";
import "./TodoList.css";

/** Rows the overview box shows at most, and how long a reordered row takes to slide to its place. */
const WINDOW_ROWS = 5;
const SLIDE_MS = 250;

/** The item being worked on: the first in progress, else the first still to do; -1 when all are done. */
export function currentTodo(items: TodoItem[]): number {
  const working = items.findIndex(item => item.status === "in_progress");
  return working >= 0 ? working : items.findIndex(item => item.status === "pending");
}

export const doneCount = (items: TodoItem[]) => items.filter(item => item.status === "completed").length;

const RANK: Record<TodoItem["status"], number> = { in_progress: 0, pending: 1, completed: 2 };

/** The list as both views show it: in progress first, then pending items, then finished ones
 *  (done, confirmed, cancelled or answered), each group in the agent's plan order. */
export const sortTodos = (items: TodoItem[]) =>
  items.map((item, at) => ({ item, at })).sort((a, b) => RANK[a.item.status] - RANK[b.item.status] || a.at - b.at).map(({ item }) => item);

export interface TodoWindowRows {
  /** Open items in the sorted order, at most five. */
  shown: TodoItem[];
  /** Open items folded below the rows. */
  more: number;
  /** Finished items, counted on the bottom line. */
  done: number;
  /** Every item is finished: the box says so in one line instead of rows. */
  allDone: boolean;
}

/** What the overview box shows: the item in progress and what comes next, at most five rows;
 *  open items past them fold into "N more" and finished ones into "N done" at the bottom, so
 *  the rows and counts always add up to the whole list. */
export function todoWindow(items: TodoItem[]): TodoWindowRows {
  const open = sortTodos(items.filter(item => item.status !== "completed"));
  const shown = open.slice(0, WINDOW_ROWS);
  return { shown, more: open.length - shown.length, done: items.length - open.length, allDone: items.length > 0 && open.length === 0 };
}

/** Slides rows (children with a `data-key`) from where they were to their new place after a
 *  reorder (FLIP); a row that just arrived doesn't move, and reduced motion moves nothing. */
function useSlide<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const tops = useRef(new Map<string, number>());
  useLayoutEffect(() => {
    const rows = [...(ref.current?.querySelectorAll<HTMLElement>(":scope > [data-key]") ?? [])];
    const was = tops.current;
    const now = new Map(rows.map(row => [row.dataset.key!, row.getBoundingClientRect().top]));
    tops.current = now;
    if (was.size === 0 || window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches) return;
    for (const row of rows) {
      const before = was.get(row.dataset.key!);
      const dy = before === undefined ? 0 : before - now.get(row.dataset.key!)!;
      if (!dy) continue;
      row.style.transition = "none";
      row.style.transform = `translateY(${dy}px)`;
      requestAnimationFrame(() => requestAnimationFrame(() => {
        row.style.transition = `transform ${SLIDE_MS}ms ease`;
        row.style.transform = "";
        row.addEventListener("transitionend", () => { row.style.transition = ""; }, { once: true });
      }));
    }
  });
  return ref;
}

const STATUS_WORDS: Partial<Record<TodoItem["status"], string>> = { completed: "Done: ", in_progress: "In progress: " };
const STATE_CLASS: Partial<Record<TodoItem["status"], string>> = { completed: " is-done", in_progress: " is-current" };

/** The item's class names: its status, and whether it arrived after the list was first shown. */
export const todoClass = (item: TodoItem, fresh = false) => `themis-todo-item${STATE_CLASS[item.status] ?? ""}${fresh ? " is-fresh" : ""}`;

/** How the goal check closed an item the agent left open. */
const CLOSED_WORDS: Record<NonNullable<TodoItem["closed"]>, string> = {
  confirmed: "confirmed by the check",
  cancelled: "cancelled by your later request",
  answered: "answered",
};

/** The mark, the status in words for a screen reader, the title and who added, reopened or closed it, as text. */
export function TodoLine({ item }: { item: TodoItem }) {
  const status = item.closed && item.closed !== "confirmed" ? "Closed: " : STATUS_WORDS[item.status];
  return <>
    <span className="themis-todo-mark" aria-hidden="true">{item.status === "in_progress" ? <DotSpinner /> : <span className="themis-todo-pending" />}</span>
    <span className="themis-todo-text">
      {status && <span className="themis-sr-only">{status}</span>}
      <span className="themis-todo-title">{item.title}</span>
      {item.source === "person" && <span className="themis-todo-tag"> · you asked</span>}
      {item.source === "check" && <span className="themis-todo-tag"> · added by the check</span>}
      {item.source === "goal" && <span className="themis-todo-tag"> · from the goal</span>}
      {item.reopened && <span className="themis-todo-tag is-reopened"> · reopened by the check</span>}
      {item.closed && item.status === "completed" && <span className="themis-todo-tag"> · {CLOSED_WORDS[item.closed]}</span>}
    </span>
  </>;
}

/** Ids the list held when first shown; later ones arrive with a brief highlight. */
function useArrivals(items: TodoItem[]) {
  const [first] = useState(() => new Set(items.map(item => item.id)));
  return (item: TodoItem) => !first.has(item.id);
}

function Item({ item, fresh }: { item: TodoItem; fresh: boolean }) {
  const [open, setOpen] = useState(false);
  const more = !!(item.details || item.outcome || item.reopened);
  return (
    <li className={todoClass(item, fresh)} data-key={item.id}>
      {more
        ? <button type="button" className="themis-todo-line" aria-expanded={open} onClick={() => setOpen(!open)}><TodoLine item={item} /></button>
        : <div className="themis-todo-line"><TodoLine item={item} /></div>}
      {more && open && (
        <div className="themis-todo-info">
          {item.details && <p>{item.details}</p>}
          {item.outcome && <p><span className="themis-todo-label">Outcome</span><span>{item.outcome}</span></p>}
          {item.reopened && <p className="is-reopened"><span className="themis-todo-label">Reopened</span><span>{item.reopened}</span></p>}
        </div>
      )}
    </li>
  );
}

/** The whole list, sorted by state; an item with details, an outcome or a reason it was reopened opens to show them. */
export function TodoItems({ items }: { items: TodoItem[] }) {
  const arrived = useArrivals(items);
  const list = useSlide<HTMLOListElement>();
  return (
    <ol ref={list} className="themis-todo" aria-label="To-do">
      {sortTodos(items).map(item => <Item key={item.id} item={item} fresh={arrived(item)} />)}
    </ol>
  );
}

/** The overview box's few rows, as phrasing content inside its button; a finished list is one
 *  quiet line (the To-do tab keeps the whole list). */
export function TodoWindow({ items }: { items: TodoItem[] }) {
  const arrived = useArrivals(items);
  const rows = useSlide<HTMLSpanElement>();
  const { shown, more, done, allDone } = todoWindow(items);
  if (allDone) return <span className="themis-todo is-compact"><span className="themis-todo-more is-all-done">{items.length} of {items.length} done</span></span>;
  return (
    <span ref={rows} className="themis-todo is-compact">
      {shown.map(item => <span key={item.id} data-key={item.id} className={todoClass(item, arrived(item))}><span className="themis-todo-line"><TodoLine item={item} /></span></span>)}
      {more > 0 && <span className="themis-todo-more">{more} more</span>}
      {done > 0 && <span className="themis-todo-more">{done} done</span>}
    </span>
  );
}
