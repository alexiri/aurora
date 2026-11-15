/**
 * FIX-137 conformance: every CommonMark 0.31.2 example and the GFM extension examples (tables, task
 * lists, strikethrough, extended autolinks, tag filter) go through the renderer and must match the
 * reference HTML, except the examples listed in EXPECTED with the D-227 exception that applies.
 * A new difference, or a listed example that starts passing, fails the test.
 *
 * Fixtures are the spec's own examples (spec.commonmark.org/0.31.2/spec.json; github/cmark-gfm
 * test/spec.txt, version 0.29-gfm, extension examples only) converted to JSON.
 */
import { appendFileSync } from "node:fs";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { Markdown } from "@/components/ui/markdown";

import commonmark from "./fixtures/commonmark-0.31.2.json";
import gfm from "./fixtures/gfm-extensions-0.29.json";
import { EXPECTED } from "./fixtures/markdown-expected";

type Example = { example: number; section: string; markdown: string; html: string };

/** Canonical form: tags and attributes (sorted), whitespace collapsed outside `pre`, no `id`s or our own `class`es. */
export function normalise(html: string): string {
  const doc = new DOMParser().parseFromString(`<body>${html}</body>`, "text/html");
  const IGNORE_ATTR = new Set(["id", "class", "title", "rel", "target", "data-footnote-ref", "data-footnote-backref", "aria-describedby", "aria-label", "loading", "alt", "role", "tabindex"]);
  const walk = (n: Node, inPre: boolean): string => {
    if (n.nodeType === Node.TEXT_NODE) {
      const v = n.textContent ?? "";
      return inPre ? v : v.replace(/\s+/g, " ");
    }
    if (n.nodeType !== Node.ELEMENT_NODE) return "";
    const el = n as Element;
    const tag = el.tagName.toLowerCase();
    // Our presentation wrappers (root, code block frame with its Copy bar, table scroller) are not content.
    if (tag === "div" && el.classList.contains("md-code")) return walk(el.querySelector("pre")!, inPre);
    if (tag === "div" && (el.classList.contains("md") || el.classList.contains("md-table"))) return [...el.childNodes].map((c) => walk(c, inPre)).join("");
    const attrs = [...el.attributes]
      .filter((a) => !IGNORE_ATTR.has(a.name) && !(tag === "code" && a.name === "class") && !(tag === "input"))
      .map((a) => {
        // react-markdown renders a cell's `align` as a style; sanitising prefixes fragment targets (as github.com does).
        const align = a.name === "style" ? /^text-align:\s*(\w+)/.exec(a.value) : null;
        if (align) return `align="${align[1]}"`;
        return `${a.name}="${a.name === "href" ? a.value.replace(/^#(user-content-)+/, "#") : a.value}"`;
      })
      .sort()
      .join(" ");
    const pre = inPre || tag === "pre";
    const inner = [...el.childNodes].map((c) => walk(c, pre)).join("");
    return `<${tag}${attrs ? " " + attrs : ""}>${inner}</${tag}>`;
  };
  const out = [...doc.body.childNodes].map((c) => walk(c, false)).join("");
  // Whitespace between block tags and at the edges is not significant.
  return out.replace(/>\s+</g, "><").replace(/^\s+|\s+$/g, "").replace(/\s+<\/(p|li|h[1-6]|td|th)>/g, "</$1>").replace(/<(p|li|h[1-6]|td|th)>\s+/g, "<$1>");
}

const render = (md: string) => normalise(renderToStaticMarkup(<Markdown source={md} />));

function run(name: string, examples: Example[], prefix: string) {
  // One test per example: a single test looping over all 652 took ~8 s idle and outran the 20 s
  // test timeout when the host was loaded; each example is now bounded on its own.
  describe(name, () => {
    it.each(examples.map((ex) => [`${prefix}${ex.example}`, ex] as const))("%s matches the reference HTML", (key, ex) => {
      let got: string;
      try {
        got = render(ex.markdown);
      } catch (e) {
        got = `THROW ${String(e)}`;
      }
      const ok = got === normalise(ex.html);
      const failing = !ok && !(key in EXPECTED) ? `${key} [${ex.section}] ${JSON.stringify(ex.markdown)}\n  want ${normalise(ex.html)}\n  got  ${got}` : "";
      // `MD_CONF_DUMP=/path pnpm exec vitest run tests/markdown-conformance.test.tsx` lists the differences to triage.
      if (process.env.MD_CONF_DUMP && failing) appendFileSync(process.env.MD_CONF_DUMP, failing + "\n");
      expect(failing).toBe("");
      expect(ok && key in EXPECTED, `${key} is listed as an expected difference but now passes — remove it from EXPECTED`).toBe(false);
    });
  });
}

run("CommonMark 0.31.2 spec examples", commonmark as Example[], "cm");
run("GFM spec extension examples", gfm as Example[], "gfm");
