import type { ReactNode } from "react";
import { toolDisplayName } from "../../lib/toolNames";
import type { ApprovalRequest } from "../../lib/types";
import { UNSANDBOXED_TOOL } from "../../lib/types";
import { SafeText, TokenChip, TokenList } from "./CommandTokens";

/** A plain sentence: fixed wording (strings) around untrusted names (paths, argv), which render only as escaped chips. */
export type ActionPart = string | { token: string } | { tokens: string[] } | { text: string };

const PATCH_OPS = /([\s\S]*?) \((?:create|modify|delete)\)(?:, |$)/g;

/** File-tool summaries the runtime writes (`summarize_fs_call`, `apply_patch`), read back into verbs and paths. */
const FILE_SUMMARIES: [RegExp, (match: RegExpExecArray) => ActionPart[]][] = [
  [/^write `([\s\S]*)`$/, match => ["Write ", { token: match[1] }]],
  [/^append to `([\s\S]*)`$/, match => ["Add to ", { token: match[1] }]],
  [/^delete `([\s\S]*)` \(recursive\)$/, match => ["Delete ", { token: match[1] }, " and everything in it"]],
  [/^delete `([\s\S]*)`$/, match => ["Delete ", { token: match[1] }]],
  [/^read `([\s\S]*)`$/, match => ["Read ", { token: match[1] }]],
  [/^move `([\s\S]*)` -> `([\s\S]*)`$/, match => ["Move ", { token: match[1] }, " to ", { token: match[2] }]],
  [/^copy `([\s\S]*)` -> `([\s\S]*)`$/, match => ["Copy ", { token: match[1] }, " to ", { token: match[2] }]],
  [/^create dir `([\s\S]*)`$/, match => ["Create the folder ", { token: match[1] }]],
  [/^apply patch to \d+: ([\s\S]+)$/, match => {
    const files = Array.from(match[1].matchAll(PATCH_OPS), file => file[1]);
    return files.length ? ["Edit ", { tokens: files }] : [];
  }],
];

/** The `"action"` of a raw ``call `tool` with {...}`` summary, when it is a plain word. */
function integrationAction(summary: string): string | null {
  return /"action":"([a-z_]{1,40})"/.exec(summary)?.[1] ?? null;
}

const folderName = (cwd: string) => cwd.split("/").filter(Boolean).pop() ?? cwd;

/** What the agent is about to do, as a plain sentence. Never invents: what it cannot read stays the runtime's own words. */
export function actionParts(request: ApprovalRequest, label?: string): ActionPart[] {
  const { command, summary, tool } = request;
  if (command) {
    const where: ActionPart[] = command.cwd !== undefined ? [" in ", { token: folderName(command.cwd) }] : [" in the project root"];
    return ["Run ", { tokens: command.argv }, ...where, command.background ? " in the background" : ""];
  }
  for (const [pattern, parts] of FILE_SUMMARIES) {
    const match = pattern.exec(summary);
    if (match && parts(match).length) return parts(match);
  }
  if (tool === "manage_integrations" || tool.startsWith("manage_integrations_")) {
    const action = integrationAction(summary);
    return [action ? `Change an integration: ${action.replace(/_/g, " ")}` : "Change an integration"];
  }
  if (tool.startsWith("mcp_start_")) return ["Connect an MCP server"];
  if (tool.startsWith("mcp_")) return [`Use ${toolDisplayName(tool, label)}`];
  // A raw argument dump is detail, not a sentence: name the tool and keep the dump for Details.
  if (summary.startsWith("call `")) return [`Use ${toolDisplayName(tool, label)}`];
  return [{ text: summary.charAt(0).toUpperCase() + summary.slice(1) }];
}

export function ActionSentence({ parts }: { parts: ActionPart[] }): ReactNode {
  return <>{parts.map((part, index) => {
    if (typeof part === "string") return part;
    if ("token" in part) return <TokenChip key={index} token={part.token} />;
    if ("tokens" in part) return <TokenList key={index} argv={part.tokens} />;
    return <SafeText key={index} text={part.text} />;
  })}</>;
}

/** One short line for what makes the step risky; only what applies. */
export function riskWords(request: ApprovalRequest): string[] {
  const words: string[] = [];
  if (request.risk === "destructive") words.push("Destructive");
  if (request.tool === UNSANDBOXED_TOOL) words.push("Outside the sandbox");
  if (request.risk === "network") words.push("Uses the network");
  if (request.sensitive_files?.length) words.push("Touches secret files");
  if (request.risk === "write") words.push("Makes changes");
  return words;
}
