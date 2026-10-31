import type { Goal, GoalRequirement } from "../lib/types";
import "./GoalView.css";

function Column({ title, items }: { title: string; items: GoalRequirement[] }) {
  return (
    <section className="themis-goal-column">
      <h3>{title} <span>{items.length}</span></h3>
      <ul aria-label={`${title}, ${items.length}`}>
        {items.map(item => (
          <li key={item.what}>
            <span>{item.what}</span>
            {item.how && <small>{item.how}</small>}
          </li>
        ))}
      </ul>
    </section>
  );
}

/** The Workspace's Goal tab: what the goal asks for, as a board. The goal check fills it and
 *  it changes nothing itself. */
export function GoalView({ goal }: { goal: Goal | null }) {
  if (!goal) return <p className="themis-goal-view-empty">This chat has no goal.</p>;
  const items = goal.proof ?? [];
  return (
    <div className="themis-goal-view">
      <p className="themis-goal-view-text">{goal.text}</p>
      {items.length === 0
        ? <p className="themis-goal-view-empty">The checklist appears after the first check.</p>
        : (
          <div className="themis-goal-board">
            <Column title="Pending" items={items.filter(item => !item.proven)} />
            <Column title="Complete" items={items.filter(item => item.proven)} />
          </div>
        )}
    </div>
  );
}
