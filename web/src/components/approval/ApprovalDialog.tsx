import { useEffect, useRef } from "react";
import type { ApprovalDecision, ApprovalRequest, RiskLevel } from "../../lib/types";
import { UNSANDBOXED_TOOL } from "../../lib/types";
import { singleKeyShortcutAllowed } from "../../lib/keyboard";
import { SafeText, TokenChip, TokenList } from "./CommandTokens";
import { Badge, type BadgeTone } from "../primitives/Badge";
import { Button } from "../primitives/Button";
import { Dialog } from "../primitives/Dialog";
import "./ApprovalDialog.css";

export interface ApprovalDialogProps {
  request: ApprovalRequest;
  open: boolean;
  onDecide: (decision: ApprovalDecision) => void;
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
const DEBUGGING_TOOLS = new Set(["browser_console", "browser_elements", "browser_network", "browser_debugger", "browser_storage", "browser_performance", "browser_memory"]);
export function browserPermission(tool: string): "Browsing" | "Debugging" {
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
  if (site) {
    const what = site.tools === "browser" ? "every browser action on" : site.tools === "web_search" ? "searches through" : "reading pages from";
    return <>Allow in this chat covers {what} <TokenList argv={[site.site]} /> in later runs of this chat. Other sites still ask.</>;
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

export function ApprovalDialog({ request, open, onDecide }: ApprovalDialogProps) {
  const grantable = request.grantable !== false;
  const outside = request.tool === UNSANDBOXED_TOOL;
  const blocked = request.command?.sandbox_blocked;
  // The caller passes a new onDecide on every render; the shortcut uses the latest one
  // without re-registering, so the pause below counts from when this request appeared,
  // not from the last re-render (during a run the app re-renders more often than that).
  const decide = useRef(onDecide);
  useEffect(() => { decide.current = onDecide; }, [onDecide]);
  const approvalId = request.approval_id;
  useEffect(() => {
    if (!open) return;
    // Keys typed just before the dialog appeared (for example digits in the composer) must not decide it.
    const openedAt = Date.now();
    function onKeyDown(event: KeyboardEvent) {
      if (event.repeat || Date.now() - openedAt < SHORTCUT_DELAY_MS) return;
      // Digits decide the approval only when they are not being typed into the composer or another field.
      if (!singleKeyShortcutAllowed(event)) return;
      // Numbering is stable: 1 Allow once, 2 Allow in this chat, 3 Deny. Requests that
      // cannot be allowed for the chat offer only 1 and 3, so a held 2 never denies.
      const decision = event.key === "1" ? "once" : event.key === "2" && grantable ? "chat" : event.key === "3" ? "deny" : null;
      if (decision === null) return;
      // The dialog closes on this keydown and focus returns to the composer before the
      // character is inserted, so the digit would land in the next message.
      event.preventDefault();
      decide.current(decision);
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open, grantable, approvalId]);

  return (
    <Dialog
      open={open}
      // A stray click elsewhere must not decide for the person: only a button, its number
      // key or Escape (an explicit Deny) does.
      closeOnOutsideClick={false}
      title={outside ? "Run outside the sandbox?" : "Approval required"}
      onClose={() => onDecide("deny")}
    >
      <div className="themis-approval">
        <div className="themis-approval-row">
          <span className="themis-approval-tool">{outside ? "shell (outside the sandbox)" : request.tool.startsWith("manage_integrations_") ? "Manage integrations" : request.tool.startsWith("mcp_start_") ? "Connect MCP" : request.tool}</span>
          <Badge tone={RISK_TONE[request.risk]}>{request.risk}</Badge>
        </div>
        {request.command
          ? <p className="themis-approval-summary">
              Run <TokenList argv={request.command.argv} />
              {request.command.cwd !== undefined ? <> in <TokenChip token={request.command.cwd} /></> : " in the project root"}
              {request.command.background && " in the background"}
            </p>
          : <p className="themis-approval-summary">{request.summary}</p>}
        {outside && <p className="themis-approval-summary">The sandbox blocked it{blocked ? <>: <SafeText text={blocked} />.</> : "."} Outside the sandbox it can write anywhere you can, read secret folders and use the network.</p>}
        <p className="themis-thread-hint"><GrantHint request={request} /></p>
        <div className="themis-approval-actions">
          <Button variant="primary" onClick={() => onDecide("once")}>
            Allow once (1)
          </Button>
          {grantable && (
            <Button variant="ghost" onClick={() => onDecide("chat")}>
              {request.grant_site?.tools === "browser" ? "Always allow on this site (2)" : "Allow in this chat (2)"}
            </Button>
          )}
          <Button variant="danger" onClick={() => onDecide("deny")}>
            Deny (3)
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
