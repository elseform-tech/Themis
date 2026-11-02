import { useState } from "react";
import { DotSpinner } from "../components/DotSpinner";
import type { TodoItem } from "../lib/types";
import "./TodoList.css";

/** Rows the overview box shows at most, and how many before the current item stay in view. */
const WINDOW_ROWS = 5;
const WINDOW_BEFORE = 2;

/** The item being worked on: the first in progress, else the first still to do; -1 when all are done. */
export function currentTodo(items: TodoItem[]): number {
  const working = items.findIndex(item => item.status === "in_progress");
  return working >= 0 ? working : items.findIndex(item => item.status === "pending");
}

export const doneCount = (items: TodoItem[]) => items.filter(item => item.status === "completed").length;

/** What the overview box shows: the current item with up to two before it, at most five rows;
 *  finished items before them fold into "N done" and the rest into "N more". */
export function todoWindow(items: TodoItem[]): { foldedDone: number; shown: TodoItem[]; more: number } {
  const current = currentTodo(items);
  const from = Math.max(0, (current >= 0 ? current : items.length) - WINDOW_BEFORE);
  const shown = items.slice(from, from + WINDOW_ROWS);
  return { foldedDone: doneCount(items.slice(0, from)), shown, more: items.length - from - shown.length };
}

const STATUS_WORDS: Partial<Record<TodoItem["status"], string>> = { completed: "Done: ", in_progress: "In progress: " };
const STATE_CLASS: Partial<Record<TodoItem["status"], string>> = { completed: " is-done", in_progress: " is-current" };

/** The item's class names: its status, and whether it arrived after the list was first shown. */
export const todoClass = (item: TodoItem, fresh = false) => `themis-todo-item${STATE_CLASS[item.status] ?? ""}${fresh ? " is-fresh" : ""}`;

/** The mark, the status in words for a screen reader, the title and who added or reopened it, as text. */
export function TodoLine({ item }: { item: TodoItem }) {
  return <>
    <span className="themis-todo-mark" aria-hidden="true">{item.status === "in_progress" ? <DotSpinner /> : <span className="themis-todo-pending" />}</span>
    <span className="themis-todo-text">
      {STATUS_WORDS[item.status] && <span className="themis-sr-only">{STATUS_WORDS[item.status]}</span>}
      <span className="themis-todo-title">{item.title}</span>
      {item.source === "person" && <span className="themis-todo-tag"> · you asked</span>}
      {item.source === "check" && <span className="themis-todo-tag"> · added by the check</span>}
      {item.reopened && <span className="themis-todo-tag is-reopened"> · reopened by the check</span>}
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
    <li className={todoClass(item, fresh)}>
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

/** The whole list; an item with details, an outcome or a reason it was reopened opens to show them. */
export function TodoItems({ items }: { items: TodoItem[] }) {
  const arrived = useArrivals(items);
  return (
    <ol className="themis-todo" aria-label="To-do">
      {items.map(item => <Item key={item.id} item={item} fresh={arrived(item)} />)}
    </ol>
  );
}

/** The overview box's few rows, as phrasing content inside its button. */
export function TodoWindow({ items }: { items: TodoItem[] }) {
  const arrived = useArrivals(items);
  const { foldedDone, shown, more } = todoWindow(items);
  return (
    <span className="themis-todo is-compact">
      {foldedDone > 0 && <span className="themis-todo-more">{foldedDone} done</span>}
      {shown.map(item => <span key={item.id} className={todoClass(item, arrived(item))}><span className="themis-todo-line"><TodoLine item={item} /></span></span>)}
      {more > 0 && <span className="themis-todo-more">{more} more</span>}
    </span>
  );
}
