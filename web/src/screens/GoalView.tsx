import { useState } from "react";
import type { Goal, GoalRequirement } from "../lib/types";
import "./GoalView.css";

/** Goal text longer than about three lines folds, so the tasks stay in view. */
const LONG_GOAL_CHARS = 220;

const isRule = (item: GoalRequirement) => item.kind === "rule";

function Task({ item, current, running }: { item: GoalRequirement; current: boolean; running: boolean }) {
  return (
    <li className={`themis-goal-step${item.proven ? " is-done" : current ? " is-current" : ""}`} aria-current={current ? "step" : undefined}>
      <span className="themis-goal-node" aria-hidden="true">
        {item.proven && <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="m2.5 6.5 2.5 2.5 4.5-5.5" /></svg>}
      </span>
      <div className="themis-goal-step-body">
        <span className="themis-goal-what">{item.what}</span>
        {current && <em>{running ? "Now" : "Next"}</em>}
        {item.how && (item.proven || current) && <small>{item.how}</small>}
      </div>
    </li>
  );
}

/** The Workspace's Goal tab: the tasks as one trail walked in order, with what the work must keep
 *  to set apart, because a rule can't be ticked off. The goal check fills it and it changes nothing. */
export function GoalView({ goal }: { goal: Goal | null }) {
  const [whole, setWhole] = useState(false);
  if (!goal) return <p className="themis-goal-view-empty">This chat has no goal.</p>;
  const items = goal.proof ?? [];
  const tasks = items.filter(item => !isRule(item));
  const rules = items.filter(isRule);
  const done = tasks.filter(item => item.proven).length;
  const current = tasks.find(item => !item.proven);
  const running = goal.status === "running";
  const long = goal.text.length > LONG_GOAL_CHARS;
  return (
    <div className="themis-goal-view">
      <header className="themis-goal-head">
        <p className={`themis-goal-view-text${long && !whole ? " is-folded" : ""}`}>{goal.text}</p>
        {long && <button type="button" className="themis-text-action themis-goal-more" onClick={() => setWhole(!whole)}>{whole ? "Less" : "More"}</button>}
      </header>
      {items.length === 0
        ? <p className="themis-goal-view-empty">The checklist will appear here in a moment.</p>
        : <>
          {tasks.length > 0 && <>
            <div className="themis-goal-tally" aria-hidden="true">
              <span className="themis-goal-count"><strong>{done}</strong> of {tasks.length}</span>
              <span className="themis-goal-strip">{tasks.map(item => <i key={item.what} className={item.proven ? "is-done" : item === current ? "is-current" : ""} />)}</span>
            </div>
            <ol className="themis-goal-trail" aria-label={`Tasks, ${done} of ${tasks.length} complete`}>
              {tasks.map(item => <Task key={item.what} item={item} current={item === current} running={running} />)}
            </ol>
          </>}
          {rules.length > 0 && (
            <section className="themis-goal-rules">
              <h3>How to work</h3>
              <ul aria-label="How to work">
                {rules.map(item => (
                  <li key={item.what} className={item.broken ? "is-broken" : ""}>
                    <span>{item.what}</span>
                    {item.broken && <small>{item.how ? `Not followed: ${item.how}` : "Not followed"}</small>}
                  </li>
                ))}
              </ul>
            </section>
          )}
        </>}
    </div>
  );
}
