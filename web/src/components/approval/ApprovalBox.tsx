import { toolDisplayName } from "../../lib/toolNames";
import type { ApprovalDecision, ApprovalRequest } from "../../lib/types";
import { UNSANDBOXED_TOOL } from "../../lib/types";
import { SafeText, TokenChip, TokenList } from "./CommandTokens";
import { ActionSentence, actionParts, riskWords } from "./plainAction";
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
  if (request.sensitive_files?.length) {
    return <>Allow in this chat covers these files{request.command ? " and this command" : ""} in later runs of this chat. Other files still ask.</>;
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

/** The exact call: argv chips for shell and git (never the joined summary), else the runtime's summary. */
function ExactCall({ request, label }: { request: ApprovalRequest; label?: string }) {
  if (request.command) return <TokenList argv={request.command.argv} />;
  const text = request.tool.startsWith("mcp_") ? request.summary.split(request.tool).join(toolDisplayName(request.tool, label)) : request.summary;
  return <SafeText text={text} />;
}

/** What the box keeps behind its Details disclosure: the exact call, where it runs, the secret files, why it asks and what a grant covers. */
function ApprovalDetails({ request, label }: { request: ApprovalRequest; label?: string }) {
  const { command } = request;
  const outside = request.tool === UNSANDBOXED_TOOL;
  const blocked = command?.sandbox_blocked;
  return (
    <details className="themis-approval-details">
      <summary>Details</summary>
      <dl>
        <dt>{outside ? "shell (outside the sandbox)" : toolDisplayName(request.tool, label)}</dt>
        <dd><ExactCall request={request} label={label} />{command?.background && " (in the background)"}</dd>
        {command?.cwd !== undefined && <><dt>Folder</dt><dd><TokenChip token={command.cwd} /></dd></>}
        {request.sensitive_files?.length ? <><dt>Secret files</dt><dd><TokenList argv={request.sensitive_files} /></dd></> : null}
        {outside && <><dt>Why</dt><dd>The sandbox blocked it{blocked ? <>: <SafeText text={blocked} />.</> : "."} Outside the sandbox it can write anywhere you can, read secret folders and use the network.</dd></>}
        <dt>{request.grantable === false ? "Always asks" : "In this chat"}</dt>
        <dd><GrantHint request={request} /></dd>
      </dl>
    </details>
  );
}

/** An action waiting for the person's decision, on the composer of its chat (canvas
 *  board D). Only that action waits; the chat stays readable meanwhile. Once decided it
 *  goes away and leaves nothing in the chat. Only its buttons decide it. */
export function ApprovalBox({ request, label, open, onDecide, next, onFold }: ApprovalBoxProps) {
  if (!open) return null;
  const grantable = request.grantable !== false;
  const outside = request.tool === UNSANDBOXED_TOOL;
  // A subagent's request names it, so the person knows which agent asks.
  const asker = request.subagent ? `${request.subagent.name} (subagent)` : null;
  const title = outside ? "Run outside the sandbox?" : asker ? `${asker} ${request.command ? "wants to run a command" : "needs approval"}` : "Approval needed";
  const risks = riskWords(request);
  return (
    <section className="themis-approval-box" aria-label={title}>
      <div className="themis-approval-head">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M12 3l7 3v5c0 4.5-3 8.5-7 10-4-1.5-7-5.5-7-10V6l7-3z" /></svg>
        <p className="themis-approval-title">{title}</p>
        {next && <span className="themis-approval-next">{next}</span>}
        {onFold && (
          <button type="button" className="themis-goal-icon themis-icon-tip" data-tooltip="Fold to icons" aria-label="Fold to icons" onClick={onFold}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
          </button>
        )}
      </div>
      <div className="themis-approval">
        <p className="themis-approval-summary"><ActionSentence parts={actionParts(request, label)} /></p>
        {risks.length > 0 && <p className={`themis-approval-risk${request.risk === "destructive" || outside ? " is-danger" : ""}`}>{risks.join(" · ")}</p>}
        <ApprovalDetails request={request} label={label} />
        <div className="themis-approval-actions">
          <Button variant="primary" onClick={() => onDecide("once")}>Allow once</Button>
          {grantable && (
            // What the grant covers shows while this button is hovered or focused, and under Details.
            <span className="themis-approval-grant">
              <Button variant="ghost" aria-describedby={`themis-grant-${request.approval_id}`} onClick={() => onDecide("chat")}>
                {request.grant_site?.tools === "browser" ? "Always allow on this site" : "Allow in this chat"}
              </Button>
              <span id={`themis-grant-${request.approval_id}`} role="tooltip" className="themis-thread-hint themis-approval-hint"><GrantHint request={request} /></span>
            </span>
          )}
          <Button variant="danger" onClick={() => onDecide("deny")}>Deny</Button>
        </div>
      </div>
    </section>
  );
}
