/** A Markdown syntax-tree node, as far as this plugin needs one. */
interface Node {
  type: string;
  value?: string;
  children?: Node[];
}

/**
 * A remark plugin that keeps a reply's single line breaks: plain Markdown joins the lines of a
 * paragraph, which flattens poems, addresses and short unbulleted lists. Each newline inside a
 * text run becomes a line break; code, inline code and the space around a break are left as
 * they are. One pass over the tree, so the work grows with the reply's length.
 */
export function remarkSoftBreaks() {
  return (tree: Node) => { split(tree); };
}

function split(node: Node): void {
  if (!node.children) return;
  const children: Node[] = [];
  for (const child of node.children) {
    if (child.type === "text" && child.value?.includes("\n")) {
      child.value.split(/[ \t]*\r?\n[ \t]*/).forEach((part, index) => {
        if (index > 0) children.push({ type: "break" });
        if (part) children.push({ type: "text", value: part });
      });
    } else {
      split(child);
      children.push(child);
    }
  }
  node.children = children;
}
