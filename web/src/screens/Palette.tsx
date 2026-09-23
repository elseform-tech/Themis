import { useEffect, useMemo, useState } from "react";
import { Dialog, EmptyState, Input } from "../components";
import { updateSettings } from "../lib/tauri";
import { describeError, toast, useApp } from "../state/store";
import { useNewThread, useOpenProject } from "./actions";
import "./Palette.css";

interface Entry {
  id: string;
  group: "Threads" | "Projects" | "Actions";
  label: string;
  detail?: string;
  run: () => void;
}

/** Case-insensitive subsequence score; null when not a match. */
function fuzzyScore(query: string, target: string): number | null {
  const q = query.toLowerCase();
  const t = target.toLowerCase();
  if (q === "") return 0;
  let qi = 0;
  let score = 0;
  let lastMatch = -2;
  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) {
      score += ti === lastMatch + 1 ? 2 : 1;
      lastMatch = ti;
      qi += 1;
    }
  }
  return qi === q.length ? score : null;
}

export function Palette() {
  const { state, dispatch } = useApp();
  const openProjectFlow = useOpenProject();
  const newThread = useNewThread();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);

  useEffect(() => {
    if (state.paletteOpen) {
      setQuery("");
      setActive(0);
    }
  }, [state.paletteOpen]);

  const entries: Entry[] = useMemo(() => {
    const list: Entry[] = [];
    for (const [root, threads] of Object.entries(state.threadsByProject)) {
      for (const thread of threads) {
        list.push({
          id: `thread:${thread.id}`,
          group: "Threads",
          label: thread.title,
          detail: root,
          run: () =>
            dispatch({ type: "thread/selected", projectRoot: root, threadId: thread.id }),
        });
      }
    }
    for (const project of state.projects) {
      list.push({
        id: `project:${project.root}`,
        group: "Projects",
        label: project.name,
        detail: project.root,
        run: () => dispatch({ type: "project/selected", root: project.root }),
      });
    }
    list.push(
      { id: "action:create-project", group: "Actions", label: "Create project", run: () => dispatch({ type: "ui/project-dialog", open: true }) },
      {
        id: "action:new-thread",
        group: "Actions",
        label: "New thread",
        run: () => void newThread(),
      },
      {
        id: "action:open-project",
        group: "Actions",
        label: "Open project…",
        run: () => void openProjectFlow(),
      },
      {
        id: "action:toggle-theme",
        group: "Actions",
        label: "Toggle theme",
        run: () => {
          const next = state.settings.theme === "light" ? "dark" : "light";
          updateSettings({ theme: next })
            .then((settings) =>
              dispatch({ type: "settings/patched", settings }),
            )
            .catch((error: unknown) =>
              toast(dispatch, `Theme switch failed: ${describeError(error)}`, "danger"),
            );
        },
      },
      {
        id: "action:settings",
        group: "Actions",
        label: "Open settings",
        run: () => dispatch({ type: "ui/view", view: "settings" }),
      },
    );
    return list;
  }, [state.threadsByProject, state.projects, state.settings.theme, dispatch, newThread, openProjectFlow]);

  const filtered = useMemo(() => {
    const scored: Array<{ entry: Entry; score: number }> = [];
    for (const entry of entries) {
      const score = fuzzyScore(query.trim(), `${entry.label} ${entry.detail ?? ""}`);
      if (score !== null) scored.push({ entry, score });
    }
    scored.sort((a, b) => b.score - a.score);
    return scored.map((s) => s.entry);
  }, [entries, query]);

  useEffect(() => {
    setActive(0);
  }, [query]);

  function run(entry: Entry | undefined) {
    if (entry === undefined) return;
    dispatch({ type: "ui/palette", open: false });
    entry.run();
  }

  return (
    <Dialog
      open={state.paletteOpen}
      title="Command palette"
      onClose={() => dispatch({ type: "ui/palette", open: false })}
    >
      <div className="themis-palette">
        <Input
          id="themis-palette-query"
          label="Search threads, projects, actions"
          placeholder="Type to filter…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown") {
              event.preventDefault();
              setActive((a) => Math.min(a + 1, Math.max(filtered.length - 1, 0)));
            } else if (event.key === "ArrowUp") {
              event.preventDefault();
              setActive((a) => Math.max(a - 1, 0));
            } else if (event.key === "Enter") {
              event.preventDefault();
              run(filtered[active]);
            }
          }}
        />
        <div className="themis-palette-list" role="listbox" aria-label="Results">
          {filtered.length === 0 ? (
            <EmptyState title="No matches" hint="Try a different query." />
          ) : (
            filtered.map((entry, index) => (
              <button
                key={entry.id}
                type="button"
                role="option"
                aria-selected={index === active}
                className={
                  index === active
                    ? "themis-palette-row themis-palette-row--active"
                    : "themis-palette-row"
                }
                onMouseEnter={() => setActive(index)}
                onClick={() => run(entry)}
              >
                <span className="themis-palette-group">{entry.group}</span>
                <span className="themis-palette-label">{entry.label}</span>
                {entry.detail !== undefined && (
                  <span className="themis-palette-detail">{entry.detail}</span>
                )}
              </button>
            ))
          )}
        </div>
      </div>
    </Dialog>
  );
}
