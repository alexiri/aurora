"use client";

import { Children, cloneElement, isValidElement, memo, useEffect, useMemo, useState, type ComponentProps, type ReactNode } from "react";
import ReactMarkdown, { type Components, type Options } from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import rehypeSlug from "rehype-slug";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";

import { useCopy, copyLabel } from "@/lib/use-copy";

import { rehypeAlerts, rehypeFragmentLinks, sanitizeSchema } from "./markdown-pipeline";

type Plugins = NonNullable<Options["remarkPlugins"]>;

/** Only absolute http(s) links are rendered as links by the callers that use this gate directly. */
export function safeHref(raw: string): string | null {
  try {
    const u = new URL(raw);
    return u.protocol === "https:" || u.protocol === "http:" ? u.toString() : null;
  } catch {
    return null;
  }
}

export type LinkKind = "same" | "external" | "hash" | "blocked";

function baseOrigin(): string {
  return typeof window !== "undefined" ? window.location.origin : "http://aurora.invalid";
}

/**
 * D-227(b): links are `http`, `https`, `mailto` or same-origin relative; any other scheme is not a
 * link. Same-origin links (relative paths and this dashboard's own origin) stay in the tab.
 */
export function classifyLink(href: string): { kind: LinkKind; href: string } {
  const raw = href.trim();
  if (raw === "") return { kind: "same", href: "" };
  if (raw.startsWith("#")) return { kind: "hash", href: raw };
  try {
    const base = baseOrigin();
    const u = new URL(raw, base);
    if (u.protocol !== "http:" && u.protocol !== "https:" && u.protocol !== "mailto:") return { kind: "blocked", href: raw };
    // The destination is kept as written (the browser normalises it on click); only its kind is decided here.
    if (u.protocol !== "mailto:" && u.origin === base) return { kind: "same", href: raw };
    return { kind: "external", href: raw };
  } catch {
    return { kind: "blocked", href: raw };
  }
}

const COMMON_TLDS = "com|org|net|io|dev|app|ch|edu|gov|uk|de|fr|eu|info|co|cloud|ai|me|us|ru|cn|jp|it|es|nl";
const URL_LABEL = new RegExp(
  `^(?:(?:https?://|www\\.)[^\\s/]+(?:[/?#]\\S*)?|(?:[a-z0-9-]+\\.)+(?:${COMMON_TLDS})(?::\\d+)?(?:[/?#]\\S*)?)$`,
  "i",
);

function hostOf(url: string): string | null {
  try {
    const u = new URL(/^[a-z][a-z0-9+.-]*:/i.test(url) ? url : `http://${url}`);
    return u.hostname.replace(/^www\./i, "").toLowerCase();
  } catch {
    return null;
  }
}

/** D-227(b): a link whose label looks like a URL or domain of a different host than its target. */
export function deceptiveLabelHost(label: string, href: string): string | null {
  const l = label.trim();
  if (!URL_LABEL.test(l)) return null;
  const labelHost = hostOf(l);
  const real = hostOf(href);
  if (!labelHost || !real || labelHost === real) return null;
  return real;
}

function textOf(node: ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return textOf(node.props.children);
  return "";
}

type AnchorProps = ComponentProps<"a"> & { node?: unknown };

function MdLink({ node: _node, href, children, ...rest }: AnchorProps) {
  void _node;
  if (href === undefined) return <span>{children}</span>;
  const c = classifyLink(href);
  if (c.kind === "blocked") return <span>{children}</span>;
  if (c.kind === "hash" || c.kind === "same") return (
    <a {...rest} href={c.href} className="text-link underline">
      {children}
    </a>
  );
  const real = deceptiveLabelHost(textOf(children), c.href);
  return (
    <a {...rest} href={c.href} title={c.href} target="_blank" rel="noopener noreferrer nofollow" className="text-link underline">
      {children}
      {real ? <span className="ml-1 text-xs text-muted">→ {real}</span> : null}
    </a>
  );
}

/** D-227(a): same-origin images (attachments) inline; any other image is a link, never loaded. */
function MdImg({ src, alt }: ComponentProps<"img"> & { node?: unknown }) {
  if (typeof src !== "string" || !src) return null;
  const c = classifyLink(src);
  const label = alt?.trim() || "image";
  if (c.kind === "same") {
    // eslint-disable-next-line @next/next/no-img-element -- chat attachments are same-origin, sized by the bubble
    return <img src={c.href} alt={alt ?? ""} loading="lazy" className="md-img" />;
  }
  if (c.kind === "external") {
    return (
      <a href={c.href} title={c.href} target="_blank" rel="noopener noreferrer nofollow" className="text-link underline">
        🖼 {label}
      </a>
    );
  }
  return <span>🖼 {label}</span>;
}

function CopyButton({ text }: { text: string }) {
  const { state, copy } = useCopy(1500);
  return (
    <button type="button" className="rounded px-1.5 py-0.5 text-xs text-muted hover:text-ink" onClick={() => copy(text)}>
      {copyLabel(state)}
    </button>
  );
}

function MdPre({ children }: ComponentProps<"pre"> & { node?: unknown }) {
  const code = Children.toArray(children).find((c) => isValidElement(c)) as { props?: { className?: string; children?: ReactNode } } | undefined;
  const cls = code?.props?.className ?? "";
  const lang = /language-([\w+#.-]+)/.exec(cls)?.[1] ?? "";
  const text = textOf(code?.props?.children ?? children).replace(/\n$/, "");
  return (
    <div className="md-code">
      <div className="md-code-head">
        <span className="font-mono text-xs text-muted">{lang}</span>
        <CopyButton text={text} />
      </div>
      {/* A scrollable region must be reachable by keyboard (axe: scrollable-region-focusable). */}
      <pre tabIndex={0} role="region" aria-label={lang ? `${lang} code` : "Code"}>
        {children}
      </pre>
    </div>
  );
}

function MdTable({ children }: ComponentProps<"table"> & { node?: unknown }) {
  return (
    <div className="md-table" tabIndex={0} role="region" aria-label="Table">
      <table>{children}</table>
    </div>
  );
}

/** A task-list checkbox is read-only; its accessible name is the item's own text. */
function MdLi({ node: _node, className, children, ...rest }: ComponentProps<"li"> & { node?: unknown }) {
  void _node;
  if (!className?.includes("task-list-item")) return <li className={className} {...rest}>{children}</li>;
  const parts = Children.toArray(children);
  const label = textOf(parts.filter((c) => !(isValidElement(c) && (c.type === "ul" || c.type === "ol")))).trim();
  return (
    <li className={className} {...rest}>
      {parts.map((c) => (isValidElement<{ "aria-label"?: string }>(c) && c.type === "input" ? cloneElement(c, { "aria-label": label || "Task" }) : c))}
    </li>
  );
}

const COMPONENTS: Components = { a: MdLink, img: MdImg, pre: MdPre, table: MdTable, li: MdLi };

type Extra = { remark: Plugins; rehype: Plugins };
const LOADERS = {
  math: () => import("./markdown-math"),
  highlight: () => import("./markdown-highlight"),
  emoji: () => import("./markdown-emoji"),
} as const;
type ExtraKey = keyof typeof LOADERS;
const EXTRA_ORDER: ExtraKey[] = ["emoji", "math", "highlight"];
const loaded = new Map<ExtraKey, Extra>();

/** Which lazy extras a message needs: math, highlighted code, emoji shortcodes. */
export function extrasFor(source: string): ExtraKey[] {
  const need = new Set<ExtraKey>();
  if (/\$|^[ \t]{0,3}(?:`{3,}|~{3,})[ \t]*math\b/m.test(source)) need.add("math");
  if (/^[ \t]{0,3}(?:`{3,}|~{3,})[ \t]*[\w+#.-]/m.test(source)) need.add("highlight");
  if (/:[a-z0-9_+-]+:/i.test(source)) need.add("emoji");
  return EXTRA_ORDER.filter((k) => need.has(k));
}

function useExtras(source: string): Extra[] {
  const want = extrasFor(source);
  const key = want.join(",");
  const [, bump] = useState(0);
  useEffect(() => {
    const missing = key ? (key.split(",") as ExtraKey[]).filter((k) => !loaded.has(k)) : [];
    if (missing.length === 0) return;
    let live = true;
    void Promise.all(missing.map((k) => LOADERS[k]().then((m) => loaded.set(k, m.default as unknown as Extra)))).then(() => {
      if (live) bump((n) => n + 1);
    });
    return () => {
      live = false;
    };
  }, [key]);
  const readyKey = want.filter((k) => loaded.has(k)).join(",");
  return useMemo(() => (readyKey ? readyKey.split(",").map((k) => loaded.get(k as ExtraKey)!) : []), [readyKey]);
}

/**
 * Streaming: an unclosed fence is already a code block to the end of the text (CommonMark); an
 * unfinished last table row (a `|` line that has not ended yet) is held back until it does.
 */
export function holdBackPartial(source: string): string {
  if (source.endsWith("\n")) return source;
  const nl = source.lastIndexOf("\n");
  const last = source.slice(nl + 1);
  if (/^\s*\|/.test(last) && nl >= 0 && /^\s*\|/.test(source.slice(source.lastIndexOf("\n", nl - 1) + 1, nl))) {
    return source.slice(0, nl + 1);
  }
  return source;
}

const BASE_REMARK: Plugins = [remarkGfm];
const BASE_REMARK_BREAKS: Plugins = [remarkGfm, remarkBreaks];
const BASE_REHYPE: Plugins = [rehypeRaw, rehypeSlug, [rehypeSanitize, sanitizeSchema], rehypeFragmentLinks, rehypeAlerts];
const REMARK_REHYPE = { allowDangerousHtml: true } as const;

/**
 * The one Markdown renderer (FIX-137, D-227): renders like github.com — CommonMark, GFM, footnotes,
 * alerts, emoji, highlighted code, math, GitHub's raw-HTML allowlist — except for the security
 * exceptions of D-227 (other-origin images become links, non-http(s)/mailto links are text, a
 * URL-looking label of another host shows the real host, raw HTML outside the allowlist is
 * dropped, KaTeX `trust: false`; Mermaid is shown as code, see docs/14). Output is React elements,
 * never an HTML string.
 *
 * `breaks` keeps a single newline inside a paragraph as a line break (what a person typed).
 * `streaming` holds back an unfinished table row while a reply is still arriving.
 */
export const Markdown = memo(function Markdown({
  source,
  breaks = false,
  streaming = false,
}: {
  source: string;
  breaks?: boolean;
  streaming?: boolean;
}) {
  const extras = useExtras(source);
  const remark = useMemo<Plugins>(
    () => [...(breaks ? BASE_REMARK_BREAKS : BASE_REMARK), ...extras.flatMap((e) => e.remark)],
    [breaks, extras],
  );
  const rehype = useMemo<Plugins>(() => [...BASE_REHYPE, ...extras.flatMap((e) => e.rehype)], [extras]);
  const text = (streaming ? holdBackPartial(source) : source).replace(/\r\n/g, "\n");
  return (
    <div className="md space-y-2 text-sm">
      <ReactMarkdown
        remarkPlugins={remark}
        rehypePlugins={rehype}
        remarkRehypeOptions={REMARK_REHYPE}
        components={COMPONENTS}
        urlTransform={(u) => u}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
