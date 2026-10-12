import { useEffect, useMemo, useState } from "react";
import { Dialog } from "../components";
import { updateSettings } from "../lib/tauri";
import { describeError, toast, useApp } from "../state/store";
import { useNewThread, useOpenProject } from "./actions";
import "./Palette.css";

interface Entry {
  id: string;
  group: "Chats" | "Projects" | "Actions";
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
          group: "Chats",
          label: thread.title,
          detail: state.projects.find(project => project.root === root)?.name ?? root.split(/[\\/]/).pop(),
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
        run: () => dispatch({ type: "project/selected", root: project.root }),
      });
    }
    list.push(
      { id: "action:create-project", group: "Actions", label: "Create project", run: () => dispatch({ type: "ui/project-dialog", open: true }) },
      {
        id: "action:new-thread",
        group: "Actions",
        label: "New chat",
        run: () => void newThread(),
      },
      {
        id: "action:open-project",
        group: "Actions",
        label: "Open folder…",
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

  // Without a query, show a short recent list per group instead of everything.
  const visible = query.trim() === "" ? (["Chats", "Projects", "Actions"] as const).flatMap(group => filtered.filter(entry => entry.group === group).slice(0, group === "Actions" ? 6 : 5)) : filtered.slice(0, 40);
  return (
    <Dialog
      open={state.paletteOpen}
      label="Search"
      className="themis-palette-dialog"
      onClose={() => dispatch({ type: "ui/palette", open: false })}
    >
      <div className="themis-palette">
        <label className="themis-palette-search">
          <svg width="18" height="18" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true"><circle cx="8.5" cy="8.5" r="5.5" /><path d="m12.5 12.5 4.2 4.2" /></svg>
          <input
            id="themis-palette-query"
            aria-label="Search chats, projects and actions"
            placeholder="Search chats, projects and actions"
            autoComplete="off"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setActive((a) => Math.min(a + 1, Math.max(visible.length - 1, 0)));
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setActive((a) => Math.max(a - 1, 0));
              } else if (event.key === "Enter") {
                event.preventDefault();
                run(visible[active]);
              }
            }}
          />
        </label>
        <div className="themis-palette-list" role="listbox" aria-label="Results">
          {visible.length === 0 ? (
            <p className="themis-palette-none">No matches</p>
          ) : (
            visible.map((entry, index) => (
              <div key={entry.id} role="presentation">
                {entry.group !== visible[index - 1]?.group && <p className="themis-palette-group" role="presentation">{entry.group}</p>}
                <button
                  type="button"
                  role="option"
                  aria-selected={index === active}
                  className={index === active ? "themis-palette-row themis-palette-row--active" : "themis-palette-row"}
                  onMouseEnter={() => setActive(index)}
                  onClick={() => run(entry)}
                >
                  <span className="themis-palette-label">{entry.label}</span>
                  {entry.detail !== undefined && <span className="themis-palette-detail">{entry.detail}</span>}
                </button>
              </div>
            ))
          )}
        </div>
      </div>
    </Dialog>
  );
}
