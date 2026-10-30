/** A tool's name as people read it: MCP tools by server and tool, hooks by plugin and hook, never their opaque wire id. */
export function toolDisplayName(name: string, label?: string): string {
  if (label) return label.replace(/_/g, " ");
  const hook = /^hook_(?:[gl]--(.+?)--[0-9a-f]+--)?(.+?)(?:_[0-9a-f]{16})?$/.exec(name);
  if (hook) return ["Hook", hook[1], hook[2]].filter(Boolean).join(" · ");
  const mcp = /^mcp_[0-9a-f]{24}_(.+)$/.exec(name);
  return (mcp ? `MCP · ${mcp[1]}` : name).replace(/_/g, " ");
}
