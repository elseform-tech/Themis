import { useState } from "react";
import { Badge, Button, EmptyState, Tabs } from "../components";
import { continueReviewItem, dismissReviewItem } from "../lib/tauri";
import { useApp } from "../state/store";
import {
  automationName,
  continueReview,
  dismissReview,
} from "./reviewFlow";
import "./ReviewQueue.css";

function formatTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

export function ReviewQueue() {
  const { state, dispatch } = useApp();
  const [tab, setTab] = useState<"pending" | "done">("pending");
  const [busy, setBusy] = useState<string | null>(null);

  const pending = state.reviews.filter((r) => r.status === "pending");
  const done = state.reviews.filter((r) => r.status !== "pending");

  async function openThread(reviewId: string) {
    if (busy !== null) return;
    const review = state.reviews.find((r) => r.id === reviewId);
    if (review === undefined) return;
    setBusy(reviewId);
    try {
      await continueReview(
        { continueReviewItem },
        dispatch,
        {
          threadsByProject: state.threadsByProject,
          automations: state.automations,
        },
        review,
      );
    } finally {
      setBusy(null);
    }
  }

  async function dismiss(reviewId: string) {
    if (busy !== null) return;
    setBusy(reviewId);
    try {
      await dismissReview({ dismissReviewItem }, dispatch, reviewId);
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="themis-review-queue">
      <div className="themis-review-queue-head">
        <h2 className="themis-review-queue-title">Review queue</h2>
        <Tabs
          tabs={[
            { id: "pending", label: `Pending (${pending.length})` },
            { id: "done", label: `Reviewed (${done.length})` },
          ]}
          value={tab}
          onChange={(id) => setTab(id as "pending" | "done")}
        />
      </div>

      {tab === "pending" ? (
        pending.length === 0 ? (
          <EmptyState
            title="No pending reviews"
            hint="Finished automation runs appear here for review."
          />
        ) : (
          <ul className="themis-review-list">
            {pending.map((item) => (
              <li key={item.id} className="themis-review-card">
                <div className="themis-review-card-head">
                  <span className="themis-review-name">{item.title}</span>
                  <Badge tone="warning">pending</Badge>
                </div>
                <p className="themis-review-summary">{item.summary}</p>
                <p className="themis-review-meta">
                  {automationName(state.automations, item.automation_id)} ·{" "}
                  {formatTime(item.created_at)}
                </p>
                <div className="themis-review-actions">
                  <Button
                    variant="primary"
                    size="small"
                    disabled={busy !== null}
                    onClick={() => void openThread(item.id)}
                  >
                    {busy === item.id ? "Opening…" : "Open thread"}
                  </Button>
                  <Button
                    variant="ghost"
                    size="small"
                    disabled={busy !== null}
                    onClick={() => void dismiss(item.id)}
                  >
                    Dismiss
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )
      ) : done.length === 0 ? (
        <EmptyState
          title="Nothing reviewed yet"
          hint="Continued and dismissed items are listed here."
        />
      ) : (
        <ul className="themis-review-list">
          {done.map((item) => (
            <li
              key={item.id}
              className="themis-review-card themis-review-card--done"
            >
              <div className="themis-review-card-head">
                <span className="themis-review-name">{item.title}</span>
                {item.status === "continued" ? (
                  <Badge tone="success">continued</Badge>
                ) : (
                  <Badge tone="default">dismissed</Badge>
                )}
              </div>
              <p className="themis-review-summary">{item.summary}</p>
              <p className="themis-review-meta">
                {automationName(state.automations, item.automation_id)} ·{" "}
                {formatTime(item.created_at)}
              </p>
              {item.status === "continued" && (
                <div className="themis-review-actions">
                  <Button
                    variant="ghost"
                    size="small"
                    disabled={busy !== null}
                    onClick={() => void openThread(item.id)}
                  >
                    {busy === item.id ? "Opening…" : "Open thread"}
                  </Button>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
