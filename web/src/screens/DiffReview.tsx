import { useCallback, useEffect, useState } from "react";
import {
  Button,
  Dialog,
  DiffView,
  EmptyState,
  FileTree,
  Input,
} from "../components";
import {
  acceptFile,
  addComment,
  discardFile,
  listDiff,
} from "../lib/tauri";
import {
  describeError,
  toast,
  useActiveThread,
  useApp,
} from "../state/store";
import { useDiscardThread, useMergeThread } from "./actions";
import "./DiffReview.css";

export function DiffReview() {
  const { state, dispatch } = useApp();
  const thread = useActiveThread();
  const threadId = thread?.id ?? null;
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [commentDraft, setCommentDraft] = useState("");
  const [acting, setActing] = useState(false);
  const [merging, setMerging] = useState(false);
  const [discarding, setDiscarding] = useState(false);
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const { merge } = useMergeThread();
  const { discard } = useDiscardThread();

  const diff = threadId === null ? undefined : state.diffs[threadId];
  const running = threadId === null ? false : (state.running[threadId] ?? false);
  const lastMerge = threadId === null ? undefined : state.merges[threadId];
  const noWorktree = thread?.worktree_path === null;
  const worktreeBusy = running || merging || discarding;
  const projectRoot = state.activeProjectRoot;

  async function doMerge() {
    if (threadId === null || projectRoot === null || worktreeBusy) return;
    setMerging(true);
    try {
      await merge(projectRoot, threadId);
    } finally {
      setMerging(false);
    }
  }

  async function doDiscard() {
    if (threadId === null || projectRoot === null || worktreeBusy) return;
    setDiscarding(true);
    try {
      const ok = await discard(projectRoot, threadId);
      if (ok) setConfirmDiscard(false);
    } finally {
      setDiscarding(false);
    }
  }

  function worktreeActions() {
    return (
      <>
        <Button
          variant="primary"
          size="small"
          disabled={worktreeBusy || noWorktree}
          title={
            noWorktree
              ? "No worktree to merge"
              : running
                ? "Wait for the run to finish"
                : "Merge worktree into your checkout"
          }
          onClick={() => void doMerge()}
        >
          {merging ? "Merging…" : "Merge"}
        </Button>
        <Button
          variant="ghost"
          size="small"
          disabled={worktreeBusy}
          title={running ? "Wait for the run to finish" : "Discard this thread"}
          onClick={() => setConfirmDiscard(true)}
        >
          Discard
        </Button>
      </>
    );
  }

  function mergeStatus() {
    if (lastMerge === undefined) return null;
    if (!lastMerge.applied) {
      return (
        <div className="themis-diffreview-conflicts" role="alert">
          <p className="themis-diffreview-subtitle">
            Merge blocked — {lastMerge.conflicts.length} conflict(s), nothing
            was applied:
          </p>
          <ul className="themis-diffreview-conflict-list">
            {lastMerge.conflicts.map((path) => (
              <li key={path} title={path}>
                {path}
              </li>
            ))}
          </ul>
          <p className="themis-diffreview-none">
            Resolve in your checkout, then discard the thread.
          </p>
        </div>
      );
    }
    return (
      <p className="themis-diffreview-merged" role="status">
        {lastMerge.files.length === 0
          ? "Merge applied — no file changes."
          : `Merge applied: ${lastMerge.files.join(", ")}`}
      </p>
    );
  }

  function discardDialog() {
    return (
      <Dialog
        open={confirmDiscard}
        title="Discard thread?"
        onClose={() => {
          if (!discarding) setConfirmDiscard(false);
        }}
      >
        <div className="themis-diffreview-discard">
          <p>
            This removes the thread
            {thread?.branch != null && (
              <> and its worktree branch <code>{thread.branch}</code> </>
            )}
            permanently. Unmerged work will be lost.
          </p>
          <div className="themis-diffreview-discard-actions">
            <Button
              variant="ghost"
              disabled={discarding}
              onClick={() => setConfirmDiscard(false)}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={discarding}
              onClick={() => void doDiscard()}
            >
              {discarding ? "Discarding…" : "Discard thread"}
            </Button>
          </div>
        </div>
      </Dialog>
    );
  }

  const refresh = useCallback(async () => {
    if (threadId === null) return;
    setLoading(true);
    try {
      const next = await listDiff(threadId);
      dispatch({ type: "diff/set", threadId, diff: next });
      if (next.available && next.files.length > 0) {
        setSelectedPath((prev) =>
          prev !== null && next.files.some((f) => f.path === prev)
            ? prev
            : (next.files[0]?.path ?? null),
        );
      } else {
        setSelectedPath(null);
      }
    } catch (error: unknown) {
      toast(dispatch, `Diff load failed: ${describeError(error)}`, "danger");
    } finally {
      setLoading(false);
    }
  }, [threadId, dispatch]);

  // Load once per thread; run-finished refreshes arrive via the store.
  useEffect(() => {
    setSelectedPath(null);
    setCommentDraft("");
    if (threadId !== null && state.diffs[threadId] === undefined) {
      void refresh();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadId]);

  async function act(path: string, kind: "accept" | "discard") {
    if (threadId === null) return;
    setActing(true);
    try {
      if (kind === "accept") await acceptFile(threadId, path);
      else await discardFile(threadId, path);
      toast(
        dispatch,
        `${kind === "accept" ? "Accepted" : "Discarded"} ${path}`,
        "success",
      );
      await refresh();
    } catch (error: unknown) {
      toast(dispatch, `${kind} failed: ${describeError(error)}`, "danger");
    } finally {
      setActing(false);
    }
  }

  async function submitComment() {
    if (threadId === null || selectedPath === null) return;
    const comment = commentDraft.trim();
    if (comment === "") return;
    setActing(true);
    try {
      const saved = await addComment(threadId, selectedPath, comment);
      dispatch({ type: "comment/added", threadId, comment: saved });
      setCommentDraft("");
      toast(dispatch, "Comment saved", "success");
    } catch (error: unknown) {
      toast(dispatch, `Comment failed: ${describeError(error)}`, "danger");
    } finally {
      setActing(false);
    }
  }

  if (threadId === null) {
    return (
      <div className="themis-diffreview">
        <EmptyState title="No thread" hint="Select a thread to review its diff." />
      </div>
    );
  }

  if (diff === undefined) {
    return (
      <div className="themis-diffreview">
        <EmptyState
          title={loading ? "Loading diff…" : "No diff loaded"}
          hint="The diff refreshes after each run."
        />
      </div>
    );
  }

  if (diff === null || !diff.available) {
    return (
      <div className="themis-diffreview">
        <div className="themis-diffreview-head">
          <h2 className="themis-diffreview-title">Diff review</h2>
          <Button
            variant="ghost"
            size="small"
            disabled={loading}
            onClick={() => void refresh()}
          >
            {loading ? "Refreshing…" : "Refresh"}
          </Button>
          {worktreeActions()}
        </div>
        {mergeStatus()}
        <EmptyState
          title="Diff unavailable"
          hint={
            diff === null
              ? "The backend returned no diff."
              : (diff.reason ?? "The backend could not produce a diff.")
          }
        />
        {discardDialog()}
      </div>
    );
  }

  const selected =
    selectedPath === null
      ? null
      : (diff.files.find((f) => f.path === selectedPath) ?? null);
  const comments =
    threadId === null ? [] : (state.comments[threadId] ?? []);
  const selectedComments =
    selectedPath === null
      ? []
      : comments.filter((c) => c.path === selectedPath);

  return (
    <div className="themis-diffreview">
      <div className="themis-diffreview-head">
        <h2 className="themis-diffreview-title">Diff review</h2>
        <Button
          variant="ghost"
          size="small"
          disabled={loading}
          onClick={() => void refresh()}
        >
          {loading ? "Refreshing…" : "Refresh"}
        </Button>
        {worktreeActions()}
      </div>
      {mergeStatus()}
      <div className="themis-diffreview-body">
        <FileTree
          files={diff.files}
          selectedPath={selectedPath}
          onSelect={setSelectedPath}
          onAccept={(path) => void act(path, "accept")}
          onDiscard={(path) => void act(path, "discard")}
          disabled={acting}
        />
        {selected === null ? (
          <EmptyState title="No file selected" hint="Pick a file to inspect." />
        ) : (
          <div className="themis-diffreview-file">
            <DiffView file={selected} />
            <div className="themis-diffreview-comments">
              <h3 className="themis-diffreview-subtitle">
                Comments ({selectedComments.length})
              </h3>
              {selectedComments.length === 0 ? (
                <p className="themis-diffreview-none">No comments on this file.</p>
              ) : (
                <ul className="themis-diffreview-comment-list">
                  {selectedComments.map((c) => (
                    <li key={c.id} className="themis-diffreview-comment">
                      {c.comment}
                    </li>
                  ))}
                </ul>
              )}
              <div className="themis-diffreview-comment-form">
                <Input
                  id="themis-diff-comment"
                  label="New comment"
                  placeholder="Leave a review note…"
                  value={commentDraft}
                  disabled={acting}
                  onChange={(event) => setCommentDraft(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void submitComment();
                  }}
                />
                <Button
                  variant="primary"
                  size="small"
                  disabled={acting || commentDraft.trim() === ""}
                  onClick={() => void submitComment()}
                >
                  Comment
                </Button>
              </div>
            </div>
          </div>
        )}
      </div>
      {discardDialog()}
    </div>
  );
}
