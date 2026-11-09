import type { ReactNode } from "react";

/** Only absolute http(s) links are rendered as links; everything else stays text. */
export function safeHref(raw: string): string | null {
  try {
    const u = new URL(raw);
    return u.protocol === "https:" || u.protocol === "http:" ? u.toString() : null;
  } catch {
    return null;
  }
}

const INLINE = /(\[([^\]]+)\]\(([^)\s]+)\)|\*\*([^*]+)\*\*|`([^`]+)`)/g;

function inline(text: string, key: string): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let i = 0;
  for (const m of text.matchAll(INLINE)) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const k = `${key}-${i++}`;
    if (m[2] !== undefined) {
      const href = safeHref(m[3]);
      out.push(
        href ? (
          <a key={k} href={href} target="_blank" rel="noopener noreferrer nofollow" className="text-link underline">
            {m[2]} <span className="text-xs text-muted">({href})</span>
          </a>
        ) : (
          <span key={k}>{m[2]}</span>
        ),
      );
    } else if (m[4] !== undefined) out.push(<strong key={k}>{m[4]}</strong>);
    else out.push(<code key={k} className="rounded bg-subtle px-1">{m[5]}</code>);
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/**
 * Minimal markdown: headings, paragraphs, lists, code blocks, bold, code, links. Raw HTML is shown as text.
 * `breaks` (GFM-style) keeps a single newline inside a paragraph as a line break; used for what a person typed.
 */
export function Markdown({ source, breaks = false }: { source: string; breaks?: boolean }) {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let i = 0;
  let n = 0;
  while (i < lines.length) {
    const line = lines[i];
    const key = `b${n++}`;
    if (line.trim() === "") {
      i++;
    } else if (line.startsWith("```")) {
      const code: string[] = [];
      i++;
      while (i < lines.length && !lines[i].startsWith("```")) code.push(lines[i++]);
      i++;
      blocks.push(<pre key={key} className="overflow-x-auto rounded bg-subtle p-2 text-xs"><code>{code.join("\n")}</code></pre>);
    } else if (/^#{1,6}\s/.test(line)) {
      const level = line.match(/^#+/)![0].length;
      const text = line.replace(/^#+\s+/, "");
      blocks.push(
        <p key={key} role="heading" aria-level={Math.min(level + 2, 6)} className="font-semibold">
          {inline(text, key)}
        </p>,
      );
      i++;
    } else if (/^\s*([-*]|\d+\.)\s/.test(line)) {
      const ordered = /^\s*\d+\./.test(line);
      const items: string[] = [];
      while (i < lines.length && /^\s*([-*]|\d+\.)\s/.test(lines[i])) items.push(lines[i++].replace(/^\s*([-*]|\d+\.)\s+/, ""));
      const children = items.map((t, j) => <li key={j}>{inline(t, `${key}-${j}`)}</li>);
      blocks.push(
        ordered ? (
          <ol key={key} className="list-decimal pl-5">{children}</ol>
        ) : (
          <ul key={key} className="list-disc pl-5">{children}</ul>
        ),
      );
    } else {
      const para: string[] = [];
      while (i < lines.length && lines[i].trim() !== "" && !lines[i].startsWith("```") && !/^#{1,6}\s/.test(lines[i]) && !/^\s*([-*]|\d+\.)\s/.test(lines[i])) para.push(lines[i++]);
      blocks.push(
        <p key={key}>
          {breaks
            ? para.map((l, j) => (
                <span key={j}>
                  {j > 0 ? <br /> : null}
                  {inline(l, `${key}-${j}`)}
                </span>
              ))
            : inline(para.join(" "), key)}
        </p>,
      );
    }
  }
  return <div className="space-y-2 text-sm">{blocks}</div>;
}
