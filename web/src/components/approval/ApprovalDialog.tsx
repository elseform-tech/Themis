import { useEffect } from "react";
import type { ApprovalDecision, ApprovalRequest, RiskLevel } from "../../lib/types";
import { singleKeyShortcutAllowed } from "../../lib/keyboard";
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

const MAX_TOKENS = 12;
const MAX_TOKEN_CHARS = 80;
// Control, format (bidi, zero-width), separator and other non-printing characters.
const HIDDEN = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Zs}\p{Co}\p{Cn}]/gu;

/** One argv token made safe to show: hidden characters become visible U+ markers, long tokens are cut, whitespace-bearing tokens are quoted. */
export function displayToken(token: string): string {
  const chars = Array.from(token);
  const cut = chars.length > MAX_TOKEN_CHARS;
  let text = chars.slice(0, MAX_TOKEN_CHARS).join("").replace(HIDDEN, ch =>
    ch === " " ? ch : `⟨U+${ch.codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0")}⟩`);
  const needsQuotes = token === "" || /[\s"'`]/u.test(token);
  if (needsQuotes) text = `"${text.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
  return cut ? `${text}…(+${chars.length - MAX_TOKEN_CHARS} chars)` : text;
}

/** Says exactly what "Allow for this run" grants. Sentence text is fixed; argv tokens render only as separate escaped chips. Keep in sync with `ToolAction::grant_key`. */
export function GrantHint({ request }: { request: ApprovalRequest }) {
  const scope = request.grant_scope;
  if (request.tool !== "shell" || !scope || scope.argv.length === 0) {
    return <>Allow for this run covers this tool until this run ends.</>;
  }
  const shown = scope.argv.slice(0, MAX_TOKENS);
  const hidden = scope.argv.length - shown.length;
  return (
    <>
      Allow for this run covers the command{" "}
      {shown.map((token, index) => <code key={index} className="themis-approval-token">{displayToken(token)}</code>)}
      {hidden > 0 && <span> …(+{hidden} more)</span>}
      {scope.covers_further_args && <span> and any further arguments</span>}
      {" "}until this run ends. Other commands still ask.
    </>
  );
}

export function ApprovalDialog({ request, open, onDecide }: ApprovalDialogProps) {
  useEffect(() => {
    if (!open) return;
    function onKeyDown(event: KeyboardEvent) {
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
        <p className="themis-approval-summary">{request.summary}</p>
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
