"use client";

import { memo, type ComponentProps } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

// C-1116: how a template's README.md is rendered everywhere people see one. It is text from the
// template's repository (SEC-146): nothing in it may run or load.
//  - No raw HTML at all (the shared Markdown's chat allowlist does not apply here).
//  - Images are never loaded; their alt text is shown.
//  - Links are only `https:` or `mailto:`; they open in a new tab and are marked as external.
//    Relative links (and every other scheme) are plain text.
//  - Headings are shifted under the page's own heading.

type AnchorProps = ComponentProps<"a"> & { node?: unknown };

/** Only `https:` and `mailto:` are links; a relative address is text, never resolved. */
function readmeHref(href: string | undefined): string | null {
  if (!href) return null;
  try {
    const u = new URL(href, "http://aurora.invalid");
    if (u.protocol !== "https:" && u.protocol !== "mailto:") return null;
    if (u.hostname === "aurora.invalid") return null;
    return href;
  } catch {
    return null;
  }
}

function MdLink({ node: _node, href, children }: AnchorProps) {
  void _node;
  const target = readmeHref(href);
  if (target === null) return <span>{children}</span>;
  return (
    <a href={target} title={target} target="_blank" rel="noopener noreferrer nofollow" className="text-link underline">
      {children}
      <span aria-hidden="true"> ↗</span>
      <span className="sr-only">(external link, opens in a new tab)</span>
    </a>
  );
}

/** An image is its alt text; nothing is fetched. */
function MdImg({ node: _node, alt }: ComponentProps<"img"> & { node?: unknown }) {
  void _node;
  return <span>{alt?.trim() || "image"}</span>;
}

type HeadingProps = ComponentProps<"h1"> & { node?: unknown };

/** `#` in the README becomes an `<h3>` under the page's own `<h1>`/`<h2>`; deeper ones never go past `<h6>`. */
function shifted(n: number) {
  const Tag = `h${Math.min(n + 2, 6)}` as "h3" | "h4" | "h5" | "h6";
  return function Heading({ node: _node, ...rest }: HeadingProps) {
    void _node;
    return <Tag {...rest} />;
  };
}

const COMPONENTS: Components = {
  a: MdLink,
  img: MdImg,
  h1: shifted(1),
  h2: shifted(2),
  h3: shifted(3),
  h4: shifted(4),
  h5: shifted(5),
  h6: shifted(6),
  pre: Pre,
};

/** A code block wraps instead of scrolling the page; it stays reachable for the keyboard. */
function Pre({ node: _node, children, ...rest }: ComponentProps<"pre"> & { node?: unknown }) {
  void _node;
  return (
    <pre {...rest} tabIndex={0} role="region" aria-label="Code" className="max-w-full overflow-x-auto rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink">
      {children}
    </pre>
  );
}

/**
 * The template's `README.md`, rendered the way people will see it (C-1116). The output is React
 * elements, never an HTML string; the page around it says whose text it is.
 */
export const TemplateReadme = memo(function TemplateReadme({ source }: { source: string }) {
  return (
    <div className="md space-y-2 text-sm [overflow-wrap:anywhere]" data-testid="template-readme">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={COMPONENTS}>
        {source.replace(/\r\n/g, "\n")}
      </ReactMarkdown>
    </div>
  );
});
