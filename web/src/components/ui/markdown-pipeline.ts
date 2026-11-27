/**
 * The Markdown pipeline (FIX-137, D-227): CommonMark + GFM + github.com's additions, rendered as
 * React elements (never an HTML string). Raw HTML goes parse → hast → sanitize → React, so what
 * is not on the allowlist is dropped (its text kept). Everything that is not a D-227 exception
 * must render like github.com.
 */
import { defaultSchema, type Options as SanitizeOptions } from "rehype-sanitize";
import type { Element, ElementContent, Root as HastRoot, Text } from "hast";
import type { Root as MdastRoot, Parent, PhrasingContent } from "mdast";

/**
 * GitHub's own sanitize schema (`defaultSchema` is derived from it) plus the allowlist of D-227:
 * `b i em strong s del ins sub sup kbd mark small br hr p div span details summary blockquote code
 * pre ul ol li table thead tbody tr th td h1–h6 a`. No `style`/`class` attributes from authors;
 * links only `http`, `https`, `mailto` (relative and `#` links have no protocol and pass).
 */
export const sanitizeSchema: SanitizeOptions = {
  ...defaultSchema,
  tagNames: [...new Set([...(defaultSchema.tagNames ?? []), "mark", "small", "details", "summary", "kbd", "ins", "del", "sub", "sup", "div", "span"])],
  protocols: { ...defaultSchema.protocols, href: ["http", "https", "mailto"], src: ["http", "https"] },
  attributes: {
    ...defaultSchema.attributes,
    // The math and highlight plugins run after sanitising and look for these classes only.
    code: [...(defaultSchema.attributes?.code ?? []), ["className", /^language-[\w+#.-]+$/, "math-inline", "math-display"]],
    details: ["open"],
  },
};

/**
 * The same list for text that must make the browser fetch nothing (`<Markdown images={false}>`): without `source` and
 * `picture`, whose `srcset` loads a file with no `img` to answer for it. `img` stays in the list so that the renderer
 * can say its alt text in its place; these are the only elements of the list that load anything by being shown.
 */
export const textOnlySchema: SanitizeOptions = {
  ...sanitizeSchema,
  tagNames: (sanitizeSchema.tagNames ?? []).filter((t) => t !== "source" && t !== "picture"),
};

export type AlertKind= "note" | "tip" | "important" | "warning" | "caution";
export const ALERT_TITLE: Record<AlertKind, string> = {
  note: "Note",
  tip: "Tip",
  important: "Important",
  warning: "Warning",
  caution: "Caution",
};

const MARKER = /^\s*\[!(note|tip|important|warning|caution)\][ \t]*\r?\n?/i;

/** github.com alerts: `> [!NOTE]` … becomes `<div class="md-alert md-alert-note">` with a title line. */
export function rehypeAlerts() {
  return (tree: HastRoot) => {
    const visit = (parent: Element | HastRoot) => {
      parent.children.forEach((child, i) => {
        if (child.type !== "element") return;
        if (child.tagName === "blockquote") {
          const p = child.children.find((c): c is Element => c.type === "element");
          const first = p?.tagName === "p" ? p.children[0] : undefined;
          const m = first?.type === "text" ? MARKER.exec(first.value) : null;
          if (p && first?.type === "text" && m) {
            const kind = m[1].toLowerCase() as AlertKind;
            first.value = first.value.slice(m[0].length);
            // `<br>` after the marker (breaks mode) is part of the marker line too.
            if (p.children[1]?.type === "element" && p.children[1].tagName === "br" && first.value === "") p.children.splice(1, 1);
            if (first.value === "") p.children.shift();
            if (p.children.length === 0) child.children = child.children.filter((c) => c !== p);
            const title: Element = {
              type: "element",
              tagName: "p",
              properties: { className: ["md-alert-title"] },
              children: [{ type: "text", value: ALERT_TITLE[kind] } as Text],
            };
            parent.children[i] = {
              type: "element",
              tagName: "div",
              properties: { className: ["md-alert", `md-alert-${kind}`], dataAlert: kind },
              children: [title, ...child.children] as ElementContent[],
            };
          }
        }
        const next = parent.children[i];
        if (next.type === "element") visit(next);
      });
    };
    visit(tree);
  };
}

/**
 * Sanitising prefixes every `id` with `user-content-` (anti DOM-clobbering, as github.com does) but
 * leaves `#fragment` hrefs alone; github.com's JavaScript bridges the two. Here the bridge is this
 * plugin (after sanitising): every in-page link gets the same prefix, so `[x](#heading)` and
 * footnote links land on their targets.
 */
export function rehypeFragmentLinks() {
  return (tree: HastRoot) => {
    const visit = (node: Element | HastRoot) => {
      for (const child of node.children) {
        if (child.type !== "element") continue;
        const href = child.properties?.href;
        if (child.tagName === "a" && typeof href === "string" && href.startsWith("#") && href.length > 1) {
          child.properties.href = `#user-content-${href.slice(1)}`;
        }
        visit(child);
      }
    };
    visit(tree);
  };
}

/**
 * github.com's `$…$` rule: the opening `$` is followed by a non-space, the closing `$` is preceded
 * by a non-space and not followed by a digit. `remark-math` alone would turn "costs $5 and $10"
 * into a formula; a span that breaks the rule goes back to the text it was.
 */
export function remarkGithubInlineMath() {
  return (tree: MdastRoot, file: unknown) => {
    const src = String(file);
    const walk = (node: Parent) => {
      node.children = node.children.map((child) => {
        const c = child as PhrasingContent & { value?: string; position?: { start: { offset?: number }; end: { offset?: number } } };
        if (c.type === ("inlineMath" as string)) {
          const s = c.position?.start.offset;
          const e = c.position?.end.offset;
          if (s === undefined || e === undefined) return child;
          const raw = src.slice(s, e);
          const inner = c.value ?? "";
          const bad = raw.startsWith("$$") ? false : /^\s|\s$/.test(inner) || /\d/.test(src[e] ?? "");
          if (bad) return { type: "text", value: raw } as PhrasingContent;
        } else if ("children" in c && Array.isArray((c as unknown as Parent).children)) {
          walk(c as unknown as Parent);
        }
        return child;
      }) as typeof node.children;
    };
    walk(tree);
  };
}
