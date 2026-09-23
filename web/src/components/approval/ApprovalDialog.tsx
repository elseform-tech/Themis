import { useEffect } from "react";
import type { ApprovalDecision, ApprovalRequest, RiskLevel } from "../../lib/types";
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

export function ApprovalDialog({ request, open, onDecide }: ApprovalDialogProps) {
  useEffect(() => {
    if (!open) return;
    function onKeyDown(event: KeyboardEvent) {
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
          <span className="themis-approval-tool">{request.tool}</span>
          <Badge tone={RISK_TONE[request.risk]}>{request.risk}</Badge>
        </div>
        <p className="themis-approval-summary">{request.summary}</p>
        <div className="themis-approval-actions">
          <Button variant="primary" onClick={() => onDecide("once")}>
            Allow once (1)
          </Button>
          <Button variant="ghost" onClick={() => onDecide("always")}>
            Always (2)
          </Button>
          <Button variant="danger" onClick={() => onDecide("deny")}>
            Deny (3)
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
