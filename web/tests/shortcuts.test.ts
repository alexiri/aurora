import { afterEach, describe, expect, it, vi } from "vitest";

import { approvalButton } from "@/components/shortcuts/shortcuts-host";
import { CHORD_WINDOW_MS, ChordMatcher, isPaletteKey, isTypingTarget, keysLabel, paletteBlocked, SHORTCUTS, shortcutBlocked, touchOnly } from "@/lib/shortcuts";

afterEach(() => {
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});

const letters = SHORTCUTS.filter((s) => s.expert);

describe("chords", () => {
  it("fires `g` then `a` as one shortcut and a single key at once", () => {
    const m = new ChordMatcher(letters);
    expect(m.feed("g", 0)).toBeNull();
    expect(m.feed("a", 300)?.id).toBe("go-agents");
    expect(m.feed("n", 400)?.id).toBe("new-chat");
    expect(m.feed("?", 500)?.id).toBe("sheet");
  });

  it("drops the first key after one second", () => {
    const m = new ChordMatcher(letters);
    m.feed("g", 0);
    // `a` alone is Allow once, not Go to Agents
    expect(m.feed("a", CHORD_WINDOW_MS + 1)?.id).toBe("allow-once");
  });

  it("treats `g` then an unrelated key as that key's own shortcut", () => {
    const m = new ChordMatcher(letters);
    m.feed("g", 0);
    expect(m.feed("b", 100)?.id).toBe("block");
    expect(m.feed("x", 200)).toBeNull();
  });

  it("knows the four go-to chords", () => {
    const m = new ChordMatcher(letters);
    const ids = ["a", "p", "o", "s"].map((k) => {
      m.feed("g", 0);
      return m.feed(k, 10)?.id;
    });
    expect(ids).toEqual(["go-agents", "go-approvals", "go-overview", "go-settings"]);
  });
});

describe("when a shortcut must not fire", () => {
  const key = { metaKey: false, ctrlKey: false, altKey: false, repeat: false, isComposing: false };
  const html = (s: string) => {
    document.body.innerHTML = s;
    return document.body.firstElementChild!.querySelector("[data-t]") ?? document.body.firstElementChild!;
  };

  it("never fires in an input, textarea, select or contenteditable", () => {
    for (const markup of ["<input data-t>", "<textarea data-t></textarea>", "<select data-t></select>", "<div data-t contenteditable='true'></div>"]) {
      expect(isTypingTarget(html(`<div>${markup}</div>`)), markup).toBe(true);
      expect(shortcutBlocked(key, { target: html(`<div>${markup}</div>`), dialogOpen: false, touch: false }), markup).toBe(true);
    }
  });

  it("never fires in the terminal or an editor (data-no-shortcuts), however deep", () => {
    const t = html("<div data-no-shortcuts><div><span data-t>x</span></div></div>");
    expect(isTypingTarget(t)).toBe(true);
    expect(paletteBlocked(t)).toBe(true);
  });

  it("fires on a plain element", () => {
    const t = html("<div><button data-t>ok</button></div>");
    expect(shortcutBlocked(key, { target: t, dialogOpen: false, touch: false })).toBe(false);
  });

  it("does not fire with a modifier, while a dialog is open, on a touch device or while composing", () => {
    const t = html("<div><button data-t>ok</button></div>");
    expect(shortcutBlocked({ ...key, ctrlKey: true }, { target: t, dialogOpen: false, touch: false })).toBe(true);
    expect(shortcutBlocked({ ...key, metaKey: true }, { target: t, dialogOpen: false, touch: false })).toBe(true);
    expect(shortcutBlocked({ ...key, altKey: true }, { target: t, dialogOpen: false, touch: false })).toBe(true);
    expect(shortcutBlocked(key, { target: t, dialogOpen: true, touch: false })).toBe(true);
    expect(shortcutBlocked(key, { target: t, dialogOpen: false, touch: true })).toBe(true);
    expect(shortcutBlocked({ ...key, isComposing: true }, { target: t, dialogOpen: false, touch: false })).toBe(true);
  });

  it("detects a touch-only device by the hover media query", () => {
    vi.stubGlobal("matchMedia", (q: string) => ({ matches: q === "(hover: hover)" ? false : false }));
    expect(touchOnly()).toBe(true);
    vi.stubGlobal("matchMedia", () => ({ matches: true }));
    expect(touchOnly()).toBe(false);
  });

  it("lets ⌘K / Ctrl K through inside a text field but not inside the terminal", () => {
    const field = html("<div><input data-t></div>");
    expect(paletteBlocked(field)).toBe(false);
    expect(isPaletteKey({ key: "k", metaKey: true, ctrlKey: false, altKey: false, shiftKey: false })).toBe(true);
    expect(isPaletteKey({ key: "K", metaKey: false, ctrlKey: true, altKey: false, shiftKey: false })).toBe(true);
    expect(isPaletteKey({ key: "k", metaKey: true, ctrlKey: false, altKey: false, shiftKey: true })).toBe(false);
    expect(isPaletteKey({ key: "k", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false })).toBe(false);
  });
});

describe("labels", () => {
  it("writes ⌘K on a Mac and Ctrl K elsewhere", () => {
    expect(keysLabel(["mod", "k"], true)).toBe("⌘K");
    expect(keysLabel(["mod", "k"], false)).toBe("Ctrl K");
    expect(keysLabel(["g", "a"], false)).toBe("g then a");
  });
});

describe("a / b answer the approval that has the focus", () => {
  it("finds the card's own button, and nothing outside a card or on a disabled one", () => {
    document.body.innerHTML = `
      <li data-approval-card id="c1"><button data-approval-key="allow-once" id="ok">Allow once</button><button data-approval-key="block" disabled id="no">Block</button></li>
      <p id="outside">x</p>`;
    expect(approvalButton(document.getElementById("c1"), "allow-once")?.id).toBe("ok");
    expect(approvalButton(document.getElementById("ok"), "allow-once")?.id).toBe("ok");
    expect(approvalButton(document.getElementById("c1"), "block")).toBeNull();
    expect(approvalButton(document.getElementById("outside"), "allow-once")).toBeNull();
    expect(approvalButton(null, "allow-once")).toBeNull();
  });
});
