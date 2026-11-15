import { describe, expect, it } from "vitest";

import {
  applyFormat,
  continueList,
  emojiQuery,
  inOpenFence,
  shortcutAction,
  smartPaste,
  tsvToTable,
  type Edit,
  type FormatAction,
} from "@/components/chat/composer/format";
import { rankEmoji } from "@/components/chat/composer/emoji";

/** Applies an edit to `text` the way the textarea would: the result text and the new selection. */
function run(text: string, s: number, e: number, action: FormatAction) {
  const ed = applyFormat(text, s, e, action);
  return done(text, ed);
}
function done(text: string, ed: Edit) {
  const out = text.slice(0, ed.from) + ed.insert + text.slice(ed.to);
  return { out, sel: out.slice(ed.selStart, ed.selEnd) };
}

describe("formatting actions, with and without a selection", () => {
  it.each([
    ["bold", "**", "bold text"],
    ["italic", "_", "italic text"],
    ["strike", "~~", "strikethrough"],
    ["code", "`", "code"],
  ] as const)("%s wraps a selection and inserts a selected placeholder without one", (action, mark, placeholder) => {
    const w = run("say hello now", 4, 9, action);
    expect(w.out).toBe(`say ${mark}hello${mark} now`);
    expect(w.sel).toBe("hello");
    const p = run("ab", 1, 1, action);
    expect(p.out).toBe(`a${mark}${placeholder}${mark}b`);
    expect(p.sel).toBe(placeholder);
  });

  it("takes the markers off a selection that is already wrapped (outside or inside the selection)", () => {
    expect(run("a **hi** b", 4, 6, "bold").out).toBe("a hi b");
    expect(run("a **hi** b", 2, 8, "bold").out).toBe("a hi b");
  });

  it("inline code over several lines becomes a code block", () => {
    const r = run("one\ntwo", 0, 7, "code");
    expect(r.out).toBe("```\none\ntwo\n```");
  });

  it("link: the selection becomes the text and `url` is selected; without a selection `text` is", () => {
    const a = run("see docs", 4, 8, "link");
    expect(a.out).toBe("see [docs](url)");
    expect(a.sel).toBe("url");
    const b = run("x", 1, 1, "link");
    expect(b.out).toBe("x[text](url)");
    expect(b.sel).toBe("text");
  });

  it("code block goes on its own lines with the caret inside", () => {
    const a = run("", 0, 0, "codeblock");
    expect(a.out).toBe("```\n\n```");
    expect(applyFormat("", 0, 0, "codeblock").selStart).toBe(4);
    const b = run("intro", 5, 5, "codeblock");
    expect(b.out).toBe("intro\n```\n\n```");
    const c = run("some code", 0, 9, "codeblock");
    expect(c.out).toBe("```\nsome code\n```");
    expect(c.sel).toBe("some code");
  });

  it("line actions prefix every selected line; numbered counts up", () => {
    expect(run("a\nb", 0, 3, "bullet").out).toBe("- a\n- b");
    expect(run("a\nb", 0, 3, "numbered").out).toBe("1. a\n2. b");
    expect(run("a\nb", 0, 3, "task").out).toBe("- [ ] a\n- [ ] b");
    expect(run("a\nb", 2, 2, "quote").out).toBe("a\n> b");
    expect(run("title", 2, 2, "heading").out).toBe("### title");
    expect(run("", 0, 0, "bullet").out).toBe("- ");
  });

  it("a selection that ends at the start of a line does not touch that line", () => {
    expect(run("a\nb", 0, 2, "bullet").out).toBe("- a\nb");
  });

  it("table inserts a two-column template on its own lines", () => {
    const t = run("x", 1, 1, "table");
    expect(t.out).toBe("x\n| Column 1 | Column 2 |\n| --- | --- |\n|  |  |");
    expect(t.sel).toBe("Column 1");
  });
});

describe("shortcuts", () => {
  const k = (key: string, extra: Partial<{ shiftKey: boolean; code: string }> = {}) => shortcutAction({ key, shiftKey: false, altKey: false, ...extra });
  it("maps Ctrl+B, I, K, E and the shifted ones", () => {
    expect(k("b")).toBe("bold");
    expect(k("i")).toBe("italic");
    expect(k("k")).toBe("link");
    expect(k("e")).toBe("code");
    expect(k("X", { shiftKey: true })).toBe("strike");
    expect(k("&", { shiftKey: true, code: "Digit7" })).toBe("numbered");
    expect(k("*", { shiftKey: true, code: "Digit8" })).toBe("bullet");
    expect(k(">", { shiftKey: true, code: "Period" })).toBe("quote");
    expect(k("z")).toBeNull();
  });
});

describe("fences and lists", () => {
  it("knows when the caret is inside an unclosed fence", () => {
    const t = "intro\n```ts\nconst a = 1;\n";
    expect(inOpenFence(t, t.length)).toBe(true);
    expect(inOpenFence(`${t}\`\`\`\nafter`, t.length + 8)).toBe(false);
    expect(inOpenFence("no fence", 3)).toBe(false);
  });

  it("Shift+Enter continues a list: bullet, next number, task", () => {
    const b = "- one";
    expect(done(b, continueList(b, 5)!).out).toBe("- one\n- ");
    const n = "9. nine";
    expect(done(n, continueList(n, 7)!).out).toBe("9. nine\n10. ");
    const t = "- [x] done";
    expect(done(t, continueList(t, 10)!).out).toBe("- [x] done\n- [ ] ");
    const i = "  * nested";
    expect(done(i, continueList(i, 10)!).out).toBe("  * nested\n  * ");
  });

  it("on an empty item it removes the marker and ends the list", () => {
    const t = "- one\n- ";
    expect(done(t, continueList(t, t.length)!).out).toBe("- one\n");
    const n = "1. one\n2. ";
    expect(done(n, continueList(n, n.length)!).out).toBe("1. one\n");
  });

  it("is null for a plain line and inside a fence", () => {
    expect(continueList("plain", 5)).toBeNull();
    expect(continueList("```\n- x", 7)).toBeNull();
  });
});

describe("smart paste", () => {
  it("a URL over a selection becomes a link; without a selection it pastes as text", () => {
    const e = smartPaste("see docs here", 4, 8, "https://example.test/a?b=1")!;
    expect(done("see docs here", e).out).toBe("see [docs](https://example.test/a?b=1) here");
    expect(smartPaste("see docs", 4, 4, "https://example.test")).toBeNull();
    expect(smartPaste("x", 0, 1, "javascript:alert(1)")).toBeNull();
    expect(smartPaste("x", 0, 1, "mailto:a@b.test")).not.toBeNull();
  });

  it("spreadsheet rows become a table with the first row as the header, pipes escaped", () => {
    expect(tsvToTable("a\tb\n1\t2\n3\tx|y\n")).toBe("| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | x\\|y |");
    expect(tsvToTable("only\trow")).toBeNull();
    expect(tsvToTable("a\tb\n1")).toBeNull();
    expect(tsvToTable("just\nlines")).toBeNull();
    const e = smartPaste("hi", 2, 2, "a\tb\n1\t2")!;
    expect(done("hi", e).out).toBe("hi\n| a | b |\n| --- | --- |\n| 1 | 2 |");
  });

  it("everything else is left to the browser", () => {
    expect(smartPaste("x", 0, 0, "plain text")).toBeNull();
  });
});

describe("emoji suggestions", () => {
  it("opens on `:` and two letters at a word start, not after a letter or digit, not in code", () => {
    expect(emojiQuery("hi :sm", 6)).toEqual({ from: 3, query: "sm" });
    expect(emojiQuery(":ta", 3)).toEqual({ from: 0, query: "ta" });
    expect(emojiQuery("10:30", 5)).toBeNull();
    expect(emojiQuery("a:sm", 4)).toBeNull();
    expect(emojiQuery("hi :s", 5)).toBeNull();
    expect(emojiQuery("use `:sm", 8)).toBeNull();
    expect(emojiQuery("```\n:sm", 7)).toBeNull();
  });

  it("ranks shortcodes that start with the letters before those that contain them, at most 8", () => {
    const all = ["smile", "sweat_smile", "smirk", "grin", "wry_smile"].map((name) => ({ name, emoji: "x" }));
    expect(rankEmoji(all, "sm").map((h) => h.name)).toEqual(["smile", "smirk", "wry_smile", "sweat_smile"]);
    expect(rankEmoji(all, "sm", 2)).toHaveLength(2);
  });
});
