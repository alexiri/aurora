/**
 * (P5-MEMORY-2) A rehype step that marks search terms in rendered Markdown. It runs after sanitizing and only splits
 * text nodes: `<mark data-match>` around each occurrence (case-insensitive), and `md-match-line` on the block that holds
 * the first match, preferring a line of text over a heading. The terms are plain strings, never patterns or markup.
 */
interface HastNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
}

const LINE_TAGS = new Set(["p", "li", "td", "th", "pre", "dt", "dd"]);
const HEADING_TAGS = new Set(["h1", "h2", "h3", "h4", "h5", "h6"]);

/**
 * For the reading size: the gap after a checklist box is the box's margin, so the space after it goes (a struck-through
 * line would cross it).
 */
export function rehypeTightTasks() {
  return (tree: unknown) => {
    const walk = (node: HastNode) => {
      node.children?.forEach((child, i) => {
        const next = node.children![i + 1];
        if (child.tagName === "input" && next?.type === "text") next.value = next.value?.replace(/^ /, "");
        walk(child);
      });
    };
    walk(tree as HastNode);
  };
}

/** `text` cut at every occurrence of any term; longer terms win where they overlap. */
export function splitByTerms(text: string, terms: string[]): { text: string; mark: boolean }[] {
  const lower = text.toLowerCase();
  const wanted = [...new Set(terms.map((t) => t.toLowerCase()).filter((t) => t.trim()))].sort((a, b) => b.length - a.length);
  const parts: { text: string; mark: boolean }[] = [];
  let at = 0;
  while (at < text.length) {
    let start = -1;
    let length = 0;
    for (const t of wanted) {
      const i = lower.indexOf(t, at);
      if (i >= 0 && (start < 0 || i < start)) {
        start = i;
        length = t.length;
      }
    }
    if (start < 0) break;
    if (start > at) parts.push({ text: text.slice(at, start), mark: false });
    parts.push({ text: text.slice(start, start + length), mark: true });
    at = start + length;
  }
  if (at < text.length) parts.push({ text: text.slice(at), mark: false });
  return parts;
}

export function rehypeMarks(terms: string[]) {
  return () => (tree: unknown) => {
    let line: HastNode | null = null;
    let heading: HastNode | null = null;
    const walk = (node: HastNode, block: HastNode | null) => {
      if (!node.children) return;
      const here = node.tagName && (LINE_TAGS.has(node.tagName) || HEADING_TAGS.has(node.tagName)) ? node : block;
      const next: HastNode[] = [];
      for (const child of node.children) {
        if (child.type !== "text") {
          walk(child, here);
          next.push(child);
          continue;
        }
        const parts = splitByTerms(child.value ?? "", terms);
        if (!parts.some((p) => p.mark)) {
          next.push(child);
          continue;
        }
        if (here?.tagName && HEADING_TAGS.has(here.tagName)) heading ??= here;
        else if (here) line ??= here;
        for (const p of parts) {
          next.push(
            p.mark
              ? { type: "element", tagName: "mark", properties: { dataMatch: "true" }, children: [{ type: "text", value: p.text }] }
              : { type: "text", value: p.text },
          );
        }
      }
      node.children = next;
    };
    walk(tree as HastNode, null);
    const target: HastNode | null = line ?? heading;
    if (target) {
      const t = target as HastNode;
      const had = Array.isArray(t.properties?.className) ? (t.properties!.className as string[]) : [];
      t.properties = { ...t.properties, className: [...had, "md-match-line"] };
    }
  };
}
