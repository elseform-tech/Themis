import { toolDisplayName } from "../../lib/toolNames";
import { useEffect, useRef } from "react";
import type React from "react";
import type { ApprovalDecision, ApprovalRequest, RiskLevel } from "../../lib/types";
import { UNSANDBOXED_TOOL } from "../../lib/types";
import { isBareCharacterKey } from "../../lib/keyboard";
import { SafeText, TokenChip, TokenList } from "./CommandTokens";
import { Badge, type BadgeTone } from "../primitives/Badge";
import { Button } from "../primitives/Button";
import "./ApprovalBox.css";

export interface ApprovalBoxProps {
  request: ApprovalRequest;
  /** A readable name for an MCP tool, shown instead of its opaque wire id. */
  label?: string;
  open: boolean;
  onDecide: (decision: ApprovalDecision) => void;
  /** What waits behind this request, such as "1 question next". */
  next?: string;
  onFold?: () => void;
}

const RISK_TONE: Record<RiskLevel, BadgeTone> = {
  read: "info",
  write: "accent",
  execute: "warning",
  network: "warning",
  destructive: "danger",
};

/** Says exactly what "Allow in this chat" grants, or why it is not offered. Sentence text is fixed; argv tokens render only as separate escaped chips. Keep in sync with `ToolAction::grant_key`. */
export { displayToken } from "./CommandTokens";

/** The Settings › Browser permission a browser tool falls under. */
const DEBUGGING_TOOLS = new Set(["browser_devtools"]);
export function browserPermission(tool: string): "Browsing" | "Downloads" | "Debugging" {
  if (tool === "browser_download") return "Downloads";
  return DEBUGGING_TOOLS.has(tool) ? "Debugging" : "Browsing";
}

export function GrantHint({ request }: { request: ApprovalRequest }) {
  if (request.grantable === false) {
    return <>This kind of action always asks, so it cannot be allowed for the chat.</>;
  }
  const scope = request.grant_scope;
  if (request.tool.startsWith("mcp_")) {
    return <>Allow in this chat covers every tool of this MCP server in later runs of this chat.</>;
  }
  const site = request.grant_site;
  if (site?.tools === "browser") {
    return <>Always allow on this site saves {browserPermission(request.tool)} for <TokenList argv={[site.site]} /> in Settings › Browser, where you can change it.</>;
  }
  if (site?.tools === "web_search") {
    // The site is the search server (the built-in one is a local address), not something the person picked.
    return <>Allow in this chat covers web searches in later runs of this chat.</>;
  }
  if (site) {
    return <>Allow in this chat covers reading pages from <TokenList argv={[site.site]} /> in later runs of this chat. Other sites still ask.</>;
  }
  const outside = request.tool === UNSANDBOXED_TOOL;
  if ((request.tool !== "shell" && !outside) || !scope || scope.argv.length === 0) {
    return <>Allow in this chat covers this tool in later runs of this chat.</>;
  }
  return (
    <>
      Allow in this chat {outside ? "lets" : "covers"} the command{" "}
      <TokenList argv={scope.argv} />
      {scope.covers_further_args && <span> and any further arguments</span>}
      {outside ? " run outside the sandbox" : ""}
      {" "}in later runs of this chat. Other commands still ask.
    </>
  );
}

const SHORTCUT_DELAY_MS = 500;

/** An action waiting for the person's decision, on the composer of its chat (canvas
 *  board D). Only that action waits; the chat stays readable meanwhile. Once decided it
 *  goes away and leaves nothing in the chat. */
export function ApprovalBox({ request, label, open, onDecide, next, onFold }: ApprovalBoxProps) {
  const grantable = request.grantable !== false;
  const outside = request.tool === UNSANDBOXED_TOOL;
  const blocked = request.command?.sandbox_blocked;
  // Keys typed just before the box appeared must not decide it: the pause counts from
  // when this request appeared, not from the last re-render (a run re-renders often).
  const openedAt = useRef(Date.now());
  const approvalId = request.approval_id;
  useEffect(() => { openedAt.current = Date.now(); }, [approvalId]);
  // Digits decide only while focus is inside this box (after a click or Tab into it):
  // the box sits beside question fields and the composer, and a digit typed after a
  // missed click must never approve or deny an action.
  function onKeyDown(event: React.KeyboardEvent) {
    if (event.repeat || Date.now() - openedAt.current < SHORTCUT_DELAY_MS) return;
    // The box sits inside the composer, so the composer-wide "this is typing" rule does not
    // apply here: only a text field inside the box would make a digit typing.
    const target = event.target as Element;
    if (event.nativeEvent.isComposing || !isBareCharacterKey(event.nativeEvent)) return;
    if (target.closest("input, textarea, select, [contenteditable]:not([contenteditable='false']), [role='textbox']")) return;
    // Numbering is stable: 1 Allow once, 2 Allow in this chat, 3 Deny. Requests that
    // cannot be allowed for the chat offer only 1 and 3, so a held 2 never denies.
    const decision = event.key === "1" ? "once" : event.key === "2" && grantable ? "chat" : event.key === "3" ? "deny" : null;
    if (decision === null) return;
    // The box goes away on this keydown; the digit must not land anywhere else.
    event.preventDefault();
    onDecide(decision);
  }

  if (!open) return null;
  // A subagent's request names it, so the person knows which agent asks.
  const asker = request.subagent ? `${request.subagent.name} (subagent)` : null;
  const title = outside ? "Run outside the sandbox?" : asker ? `${asker} ${request.command ? "wants to run a command" : "needs approval"}` : "Approval needed";
  return (
    <section className="themis-approval-box" aria-label={title} tabIndex={-1} onKeyDown={onKeyDown}>
      <div className="themis-approval-head">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M12 3l7 3v5c0 4.5-3 8.5-7 10-4-1.5-7-5.5-7-10V6l7-3z" /></svg>
        <p className="themis-approval-title">{title}</p>
        <span className="themis-approval-tool">{outside ? "shell (outside the sandbox)" : request.tool.startsWith("manage_integrations_") ? "Manage integrations" : request.tool.startsWith("mcp_start_") ? "Connect MCP" : toolDisplayName(request.tool, label)}</span>
        <Badge tone={RISK_TONE[request.risk]}>{request.risk}</Badge>
        {next && <span className="themis-approval-next">{next}</span>}
        {onFold && (
          <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Fold to icons" aria-label="Fold to icons" onClick={onFold}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
          </button>
        )}
      </div>
      <div className="themis-approval">
        {request.command
          ? <p className="themis-approval-summary">
              Run <TokenList argv={request.command.argv} />
              {request.command.cwd !== undefined ? <> in <TokenChip token={request.command.cwd} /></> : " in the project root"}
              {request.command.background && " in the background"}
            </p>
          : <p className="themis-approval-summary">{request.tool.startsWith("mcp_") ? request.summary.split(request.tool).join(toolDisplayName(request.tool, label)) : request.summary}</p>}
        {outside && <p className="themis-approval-summary">The sandbox blocked it{blocked ? <>: <SafeText text={blocked} />.</> : "."} Outside the sandbox it can write anywhere you can, read secret folders and use the network.</p>}
        <div className="themis-approval-actions">
          <Button variant="primary" onClick={() => onDecide("once")}>
            Allow once (1)
          </Button>
          {grantable && (
            // What the grant covers shows only while this button is hovered or focused.
            <span className="themis-approval-grant">
              <Button variant="ghost" aria-describedby={`themis-grant-${request.approval_id}`} onClick={() => onDecide("chat")}>
                {request.grant_site?.tools === "browser" ? "Always allow on this site (2)" : "Allow in this chat (2)"}
              </Button>
              <span id={`themis-grant-${request.approval_id}`} role="tooltip" className="themis-thread-hint themis-approval-hint"><GrantHint request={request} /></span>
            </span>
          )}
          <Button variant="danger" onClick={() => onDecide("deny")}>
            Deny (3)
          </Button>
        </div>
      </div>
    </section>
  );
}
