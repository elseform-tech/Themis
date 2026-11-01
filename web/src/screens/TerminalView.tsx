import { useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal, type ITheme } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { inputChunks, terminalInput, terminalOpen, terminalResize, terminalStream } from "../lib/terminal";
import { describeError } from "../state/store";
import "./TerminalView.css";

/** The app's colours, read from its tokens: the same in the light and the dark theme. */
function terminalTheme(element: HTMLElement): ITheme {
  const styles = getComputedStyle(element);
  const token = (name: string) => styles.getPropertyValue(name).trim() || undefined;
  return {
    background: token("--themis-canvas"),
    foreground: token("--themis-text"),
    cursor: token("--themis-text"),
    cursorAccent: token("--themis-canvas"),
    selectionBackground: token("--themis-raised"),
  };
}

function endedLine(code: number | null): string {
  if (code === null) return "The shell has ended";
  return code === 0 ? "The shell has exited" : `The shell exited with code ${code}`;
}

/** Workspace › Terminal: the person's own shell in the project (the chat's worktree when it has one). The server keeps it, so leaving the tab or the chat doesn't end it. */
export function TerminalView({ projectRoot, threadId }: { projectRoot: string; threadId: string }) {
  const host = useRef<HTMLDivElement>(null);
  const [attempt, setAttempt] = useState(0);
  const [ended, setEnded] = useState<{ code: number | null } | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    const element = host.current;
    if (!element) return;
    let left = false;
    let root: string | null = null;
    let stop: (() => void) | undefined;
    const terminal = new Terminal({
      cursorBlink: false,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      fontSize: 12,
      scrollback: 5000,
      theme: terminalTheme(element),
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(element);
    fit.fit();
    terminal.focus();
    // In order, whatever the size of a paste.
    let typing = Promise.resolve();
    const typed = terminal.onData(data => {
      if (root === null) return;
      const target = root;
      for (const chunk of inputChunks(data)) {
        typing = typing.then(() => terminalInput(target, chunk)).catch(reason => { if (!left) setError(describeError(reason)); });
      }
    });
    terminalOpen(projectRoot, threadId, terminal.cols, terminal.rows).then(info => {
      if (left) return;
      root = info.root;
      stop = terminalStream(info.root, update => {
        if (left) return;
        if (update.kind === "state") { if (update.state.scrollback) terminal.write(update.state.scrollback); }
        else if (update.kind === "output") terminal.write(update.data);
        else setEnded({ code: update.code });
      }, reason => { if (!left) setError(describeError(reason)); });
    }).catch(reason => { if (!left) setError(describeError(reason)); });
    const resized = new ResizeObserver(() => {
      const { cols, rows } = terminal;
      fit.fit();
      if (root !== null && (terminal.cols !== cols || terminal.rows !== rows)) void terminalResize(root, terminal.cols, terminal.rows).catch(() => undefined);
    });
    resized.observe(element);
    // The theme can change while it is open.
    const themed = new MutationObserver(() => { terminal.options.theme = terminalTheme(element); });
    themed.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => {
      left = true;
      stop?.();
      resized.disconnect();
      themed.disconnect();
      typed.dispose();
      terminal.dispose();
    };
  }, [projectRoot, threadId, attempt]);

  return (
    <div className="themis-terminal">
      <div key={attempt} ref={host} className="themis-terminal-host" />
      {error && <p className="themis-workspace-note" role="alert">{error}</p>}
      {ended && (
        <p className="themis-terminal-ended" role="status">
          {endedLine(ended.code)}
          <button type="button" onClick={() => { setEnded(null); setError(""); setAttempt(value => value + 1); }}>Restart</button>
        </p>
      )}
    </div>
  );
}
