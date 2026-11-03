import { useState } from "react";
import type { Goal, TodoList } from "../lib/types";
import { doneCount, TodoItems } from "./TodoList";
import "./TodoView.css";

/** Goal text longer than about three lines folds, so the list stays in view. */
const LONG_GOAL_CHARS = 220;

/** The Workspace's To-do tab: the goal when there is one and the agent's whole list. It
 *  changes nothing itself. */
export function TodoView({ goal, todo }: { goal: Goal | null; todo: TodoList | undefined }) {
  const [whole, setWhole] = useState(false);
  const items = todo?.items ?? [];
  const long = !!goal && goal.text.length > LONG_GOAL_CHARS;
  return (
    <div className="themis-todo-view">
      {goal && (
        <header className="themis-todo-goal">
          <p className={`themis-todo-goal-text${long && !whole ? " is-folded" : ""}`}>{goal.text}</p>
          {long && <button type="button" className="themis-text-action themis-todo-goal-more" onClick={() => setWhole(!whole)}>{whole ? "Less" : "More"}</button>}
        </header>
      )}
      {items.length === 0
        ? <p className="themis-todo-view-empty">The agent hasn't written its list yet.</p>
        : <>
          <p className="themis-todo-tally"><strong>{doneCount(items)}</strong> of {items.length} done</p>
          <TodoItems items={items} />
        </>}
    </div>
  );
}
