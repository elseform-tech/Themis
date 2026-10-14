import { useEffect } from "react";
import type { ApprovalDecision, ApprovalRequest, RiskLevel } from "../../lib/types";
import { singleKeyShortcutAllowed } from "../../lib/keyboard";
import { TokenChip, TokenList } from "./CommandTokens";
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

export function GrantHint({ request }: { request: ApprovalRequest }) {
  if (request.grantable === false) {
    return <>This kind of action always asks, so it cannot be allowed for the chat.</>;
  }
  const scope = request.grant_scope;
  if (request.tool.startsWith("mcp_")) {
    return <>Allow in this chat covers every tool of this MCP server in later runs of this chat.</>;
  }
  if (request.tool !== "shell" || !scope || scope.argv.length === 0) {
    return <>Allow in this chat covers this tool in later runs of this chat.</>;
  }
  return (
    <>
      Allow in this chat covers the command{" "}
      <TokenList argv={scope.argv} />
      {scope.covers_further_args && <span> and any further arguments</span>}
      {" "}in later runs of this chat. Other commands still ask.
    </>
  );
}

const SHORTCUT_DELAY_MS = 500;

export function ApprovalDialog({ request, open, onDecide }: ApprovalDialogProps) {
  const grantable = request.grantable !== false;
  useEffect(() => {
    if (!open) return;
    // Keys typed just before the dialog appeared (for example digits in the composer) must not decide it.
    const openedAt = Date.now();
    function onKeyDown(event: KeyboardEvent) {
      if (event.repeat || Date.now() - openedAt < SHORTCUT_DELAY_MS) return;
      // Digits decide the approval only when they are not being typed into the composer or another field.
      if (!singleKeyShortcutAllowed(event)) return;
      // Destructive requests offer only Allow once (1) and Deny (2).
      if (event.key === "1") onDecide("once");
      else if (event.key === "2") onDecide(grantable ? "chat" : "deny");
      else if (event.key === "3" && grantable) onDecide("deny");
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open, onDecide, grantable]);

  return (
    <Dialog
      open={open}
      title="Approval required"
      onClose={() => onDecide("deny")}
    >
      <div className="themis-approval">
        <div className="themis-approval-row">
          <span className="themis-approval-tool">{request.tool.startsWith("manage_integrations_") ? "Manage integrations" : request.tool.startsWith("mcp_start_") ? "Connect MCP" : request.tool}</span>
          <Badge tone={RISK_TONE[request.risk]}>{request.risk}</Badge>
        </div>
        {request.command
          ? <p className="themis-approval-summary">
              Run <TokenList argv={request.command.argv} />
              {request.command.cwd !== undefined ? <> in <TokenChip token={request.command.cwd} /></> : " in the project root"}
              {request.command.background && " in the background"}
            </p>
          : <p className="themis-approval-summary">{request.summary}</p>}
        <p className="themis-thread-hint"><GrantHint request={request} /></p>
        <div className="themis-approval-actions">
          <Button variant="primary" onClick={() => onDecide("once")}>
            Allow once (1)
          </Button>
          {grantable && (
            <Button variant="ghost" onClick={() => onDecide("chat")}>
              Allow in this chat (2)
            </Button>
          )}
          <Button variant="danger" onClick={() => onDecide("deny")}>
            Deny ({grantable ? 3 : 2})
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
