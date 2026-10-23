/** A tool's name as people read it: MCP tools by server and tool, never their opaque wire id. */
export function toolDisplayName(name: string, label?: string): string {
  if (label) return label.replace(/_/g, " ");
  const mcp = /^mcp_[0-9a-f]{24}_(.+)$/.exec(name);
  return (mcp ? `MCP · ${mcp[1]}` : name).replace(/_/g, " ");
}
