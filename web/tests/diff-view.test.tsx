import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { DiffView } from "@/components/ui/diff-view";

// FIX-250: the shared diff component — fold rows labelled with the lines they hide, expand buttons
// that grant exactly the lines they name, keyboard-operable controls, wrapped lines, and
// added / removed / unchanged files said in words.

afterEach(cleanup);

const lines = (n: number) => Array.from({ length: n }, (_, i) => `line ${i + 1}`).join("\n");

/** 80 lines with line 50 changed: a 46-line fold before the pair and a 27-line one after. */
const big = () => {
  const old = lines(80);
  return { old, next: old.replace("line 50", "fifty") };
};

const folds = () => screen.getAllByTestId("diff-fold");
const lineRows = () => Array.from(document.querySelectorAll("tr[data-diff]"));

describe("DiffView", () => {
  // (FIX-289) Every diff wraps its long lines; no caller chooses.
  it("always wraps long lines, and no caller can turn wrapping off", () => {
    // @ts-expect-error there is no `wrap` prop (the typecheck fails if one comes back)
    render(<DiffView old="a" next={"x".repeat(300)} label="d" wrap={false} />);
    const cell = screen.getByTestId("diff-view").querySelector('[data-diff="add"] td:last-child')!;
    const text = cell.querySelector("[data-diff-text]")!;
    expect(text.className.split(" ")).toEqual(expect.arrayContaining(["whitespace-pre-wrap", "wrap-anywhere"]));
    expect(cell.className.split(" ")).not.toContain("whitespace-pre");
  });

  // (FIX-277) The sign stands in its own column and the text beside it, so a continued line starts under
  // the text, never under the sign. (The geometry is checked in e2e/fix-277.spec.ts.)
  it("puts the sign and the text side by side", () => {
    render(<DiffView old={"keep\nSwitch the bot off"} next={"keep\nSwitch the bot off by 1 December"} label="d" />);
    for (const [kind, sign, words] of [
      ["same", "  ", "keep"],
      ["del", "- ", "Switch the bot off"],
      ["add", "+ ", "Switch the bot off by 1 December"],
    ] as const) {
      const cell = document.querySelector(`tr[data-diff="${kind}"] td:last-child`)!;
      const [mark, text] = Array.from(cell.firstElementChild!.children);
      expect(mark!.textContent, kind).toBe(sign);
      expect(mark!.className.split(" "), kind).toEqual(expect.arrayContaining(["shrink-0", "whitespace-pre"]));
      expect(text!.hasAttribute("data-diff-text"), kind).toBe(true);
      expect(text!.textContent, kind).toBe(words);
      expect(cell.firstElementChild!.className.split(" "), kind).toContain("flex");
    }
  });

  it("labels the fold row with the lines it hides, and each button names its range", () => {
    render(<DiffView old={big().old} next={big().next} label="Diff of Instructions" fileName="instructions.txt" />);
    expect(screen.getByRole("region", { name: "Diff of Instructions" })).toBeTruthy();
    expect(screen.getByText("instructions.txt")).toBeTruthy();
    expect(folds()).toHaveLength(2);
    expect(folds()[0].textContent).toContain("46 unchanged lines");
    // The collapsed fold's buttons name exactly what a click grants.
    const first = within(folds()[0]);
    expect(first.getByRole("button", { name: "Show 20 more lines above (lines 1 to 20)" })).toBeTruthy();
    expect(first.getByRole("button", { name: "Show 20 more lines below (lines 27 to 46)" })).toBeTruthy();
    expect(first.getByRole("button", { name: "Show all 46 unchanged lines (lines 1 to 46)" })).toBeTruthy();
    expect(within(folds()[1]).getByRole("button", { name: "Show all 27 unchanged lines (lines 54 to 80)" })).toBeTruthy();
  });

  it("Show 20 more above grants exactly the lines it names", () => {
    render(<DiffView old={big().old} next={big().next} label="d" />);
    fireEvent.click(screen.getByRole("button", { name: "Show 20 more lines above (lines 1 to 20)" }));
    expect(folds()).toHaveLength(2);
    expect(folds()[0].textContent).toContain("26 unchanged lines");
    const texts = lineRows().map((r) => (r as HTMLTableRowElement).cells[2].textContent);
    expect(texts.slice(0, 20)).toEqual(Array.from({ length: 20 }, (_, i) => `  line ${i + 1}`));
    // The revealed lines sit above the context of the change.
    expect(texts[20]).toBe("  line 47");
  });

  it("Show 20 more below grants exactly the lines it names", () => {
    render(<DiffView old={big().old} next={big().next} label="d" />);
    fireEvent.click(screen.getByRole("button", { name: "Show 20 more lines below (lines 27 to 46)" }));
    expect(folds()[0].textContent).toContain("26 unchanged lines");
    const texts = lineRows().map((r) => (r as HTMLTableRowElement).cells[2].textContent);
    // The revealed lines are the fold's last 20, then the change's context follows.
    expect(texts.slice(0, 20)).toEqual(Array.from({ length: 20 }, (_, i) => `  line ${i + 27}`));
    expect(texts[20]).toBe("  line 47");
  });

  it("Show all expands the whole run, and the last step empties the fold", () => {
    render(<DiffView old={big().old} next={big().next} label="d" />);
    fireEvent.click(screen.getByRole("button", { name: "Show all 46 unchanged lines (lines 1 to 46)" }));
    expect(folds()).toHaveLength(1);
    expect(document.querySelectorAll("tr[data-diff]")).toHaveLength(54);
    fireEvent.click(screen.getByRole("button", { name: "Show all 27 unchanged lines (lines 54 to 80)" }));
    expect(screen.queryByTestId("diff-fold")).toBeNull();
    expect(document.querySelectorAll("tr[data-diff]")).toHaveLength(81);
  });

  it("Show 20 more above keeps granting 20 until the fold is gone", () => {
    render(<DiffView old={big().old} next={big().next} label="d" />);
    const button = () => within(folds()[0]).getByRole("button", { name: /more lines above/ });
    fireEvent.click(button());
    fireEvent.click(button());
    expect(folds()[0].textContent).toContain("6 unchanged lines");
    // The last step grants only what is left, and names it so.
    expect(within(folds()[0]).getByRole("button", { name: "Show 6 more lines above (lines 41 to 46)" })).toBeTruthy();
    fireEvent.click(button());
    expect(folds()[0].textContent).toContain("27 unchanged lines");
  });

  // (FIX-289) Each fold's Show all opens it; there is no switch for the whole file.
  it("has no Show the whole file switch", () => {
    render(<DiffView old={big().old} next={big().next} label="d" />);
    expect(screen.queryByRole("switch")).toBeNull();
    expect(screen.queryByText("Show the whole file")).toBeNull();
  });

  // (FIX-289) The fold row sits at the left of the box and wraps, so a very long line never pushes it out of view.
  // (The geometry is checked in e2e/fix-250-template-diff-sections.spec.ts.)
  it("with a 300-character line, keeps the fold row and its three buttons at the left, in reach", () => {
    const old = lines(80).replace("line 50", "y".repeat(300));
    render(<DiffView old={old} next={old.replace("y".repeat(300), "z".repeat(300))} label="d" />);
    const cell = folds()[0].querySelector("td")!;
    expect(cell.className.split(" ")).toContain("text-left");
    expect(cell.className.split(" ")).not.toContain("text-center");
    expect(cell.firstElementChild!.className.split(" ")).toEqual(expect.arrayContaining(["flex", "flex-wrap"]));
    const buttons = within(folds()[0]).getAllByRole("button");
    expect(buttons.map((b) => b.textContent)).toEqual(["Show 20 more lines above", "Show 20 more lines below", "Show all"]);
  });

  it("the fold buttons are keyboard-operable native buttons", () => {
    render(<DiffView old={big().old} next={big().next} label="d" />);
    for (const b of within(folds()[0]).getAllByRole("button")) {
      expect((b as HTMLButtonElement).tagName).toBe("BUTTON");
      expect((b as HTMLButtonElement).disabled).toBe(false);
    }
  });

  it("an added file says so and shows every line as an addition", () => {
    render(<DiffView old="" next={"x\ny"} label="d" />);
    expect(screen.getByText("(added)")).toBeTruthy();
    expect(lineRows().every((r) => r.getAttribute("data-diff") === "add")).toBe(true);
    expect(lineRows()).toHaveLength(2);
  });

  it("a removed file says so and shows every line as a removal", () => {
    render(<DiffView old={"x\ny"} next="" label="d" />);
    expect(screen.getByText("(removed)")).toBeTruthy();
    expect(lineRows().every((r) => r.getAttribute("data-diff") === "del")).toBe(true);
    expect(lineRows()).toHaveLength(2);
  });

  it("identical files are not shown as a diff at all", () => {
    render(<DiffView old={"a\nb"} next={"a\nb"} label="d" />);
    expect(screen.getByText("Unchanged.")).toBeTruthy();
    expect(screen.queryByTestId("diff-fold")).toBeNull();
    expect(lineRows()).toHaveLength(0);
  });

  it("a changed-line pair highlights the words that differ", () => {
    render(<DiffView old="return the first item" next="return the last item" label="d" />);
    const marks = Array.from(document.querySelectorAll("mark")).map((m) => m.textContent);
    expect(marks).toEqual(["first", "last"]);
  });

  it("a block's removed and added lines are paired in order for the word highlight", () => {
    render(<DiffView old={"keep\nred apple\nred pear"} next={"keep\ngreen apple\ngreen pear"} label="d" />);
    const marks = Array.from(document.querySelectorAll("mark")).map((m) => m.textContent);
    expect(marks.sort()).toEqual(["green", "green", "red", "red"]);
  });

  it("the scroll box is a region with its own name", () => {
    render(<DiffView old="a" next="b" label="Diff of Instructions" />);
    const box = screen.getByRole("region", { name: "Scrollable lines: Instructions" });
    expect(box).toBe(screen.getByTestId("diff-box"));
    expect(box.getAttribute("tabindex")).toBe("0");
  });

  it("a file too large to compare says so, with the whole old and new text in closed folds", () => {
    const old = lines(3_000);
    const next = old.replace("line 10\n", "line ten\n").replace("line 2990\n", "line late\n");
    render(<DiffView old={old} next={next} label="Diff of big" />);
    expect(screen.getByText("This file is too large to compare line by line; the whole old and new text are below.")).toBeTruthy();
    expect(screen.queryByTestId("diff-box")).toBeNull();
    expect(screen.queryByRole("switch")).toBeNull();
    const whole = screen.getAllByTestId("diff-whole-text");
    expect(whole).toHaveLength(2);
    expect(whole.map((w) => (w as HTMLDetailsElement).open)).toEqual([false, false]);
    expect(screen.getByLabelText("Old text: big").textContent).toBe(old);
    expect(screen.getByLabelText("New text: big").textContent).toBe(next);
  });

  it("an added or removed empty file says it is empty, never Unchanged", () => {
    render(<DiffView old="" next="" label="d" status="added" />);
    expect(screen.getByText("The file is empty.")).toBeTruthy();
    expect(screen.queryByText("Unchanged.")).toBeNull();
  });

  it("long lines wrap inside the box, never widening the page", () => {
    render(<DiffView old={"x".repeat(400)} next={"y".repeat(400)} label="d" />);
    const box = screen.getByTestId("diff-box");
    expect(box.className).toContain("overflow-x-auto");
    expect(box.className).toContain("max-w-full");
    expect(document.querySelectorAll("[data-diff-text]")).toHaveLength(2);
    expect(document.querySelector("td.whitespace-pre")).toBeNull();
  });
});
