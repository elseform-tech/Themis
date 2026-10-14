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

/** Says exactly what "Allow for this run" grants. Sentence text is fixed; argv tokens render only as separate escaped chips. Keep in sync with `ToolAction::grant_key`. */
export { displayToken } from "./CommandTokens";

export function GrantHint({ request }: { request: ApprovalRequest }) {
  const scope = request.grant_scope;
  if (request.tool !== "shell" || !scope || scope.argv.length === 0) {
    return <>Allow for this run covers this tool until this run ends.</>;
  }
  return (
    <>
      Allow for this run covers the command{" "}
      <TokenList argv={scope.argv} />
      {scope.covers_further_args && <span> and any further arguments</span>}
      {" "}until this run ends. Other commands still ask.
    </>
  );
}

const SHORTCUT_DELAY_MS = 500;

export function ApprovalDialog({ request, open, onDecide }: ApprovalDialogProps) {
  useEffect(() => {
    if (!open) return;
    // Keys typed just before the dialog appeared (for example digits in the composer) must not decide it.
    const openedAt = Date.now();
    function onKeyDown(event: KeyboardEvent) {
      if (event.repeat || Date.now() - openedAt < SHORTCUT_DELAY_MS) return;
      // Digits decide the approval only when they are not being typed into the composer or another field.
      if (!singleKeyShortcutAllowed(event)) return;
      if (event.key === "1") onDecide("once");
      else if (event.key === "2") onDecide("always");
      else if (event.key === "3") onDecide("deny");
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open, onDecide]);

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
              {request.command.cwd !== undefined && <> in <TokenChip token={request.command.cwd} /></>}
              {request.command.background && " in the background"}
            </p>
          : <p className="themis-approval-summary">{request.summary}</p>}
        <p className="themis-thread-hint"><GrantHint request={request} /></p>
        <div className="themis-approval-actions">
          <Button variant="primary" onClick={() => onDecide("once")}>
            Allow once (1)
          </Button>
          <Button variant="ghost" onClick={() => onDecide("always")}>
            Allow for this run (2)
          </Button>
          <Button variant="danger" onClick={() => onDecide("deny")}>
            Deny (3)
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
