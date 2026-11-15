import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Tabs, visibleTabs } from "@/components/ui/tabs";

// FIX-129: `Tabs overflow="menu"` moves the tabs that do not fit into a trailing More menu (no sideways scroll);
// the selected tab is always shown; arrow keys still move across every tab; the menu is a proper menu button.

describe("visibleTabs", () => {
  const w = [100, 100, 100, 100, 100, 100];

  it("shows every tab when they fit", () => {
    expect(visibleTabs(w, 700, 60, 0)).toEqual([0, 1, 2, 3, 4, 5]);
  });

  it("keeps the first tabs that fit beside More", () => {
    // budget = 350 - 60 - 4 = 286 → 100 + 104 = 204, + 104 = 308 > 286
    expect(visibleTabs(w, 350, 60, 0)).toEqual([0, 1]);
  });

  it("shows a selected tab that lives in the overflow in place of the last visible one", () => {
    expect(visibleTabs(w, 350, 60, 5)).toEqual([0, 5]);
  });

  it("never returns an empty strip", () => {
    expect(visibleTabs(w, 50, 60, 3)).toEqual([3]);
    expect(visibleTabs(w, 50, 60, 0)).toEqual([0]);
  });

  it("treats unmeasured (zero-width) tabs as fitting", () => {
    expect(visibleTabs([0, 0, 0], 0, 0, 1)).toEqual([0, 1, 2]);
  });
});

const items = Array.from({ length: 8 }, (_, i) => ({ value: `t${i}`, label: `Tab ${i}` }));

function Harness({ initial = "t0" }: { initial?: string }) {
  const [value, setValue] = useState(initial);
  return (
    <>
      <Tabs items={items} value={value} onValueChange={setValue} variant="underline" overflow="menu" label="Agent" idBase="x" />
      <p data-testid="current">{value}</p>
    </>
  );
}

const original = {
  offsetWidth: Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetWidth"),
  clientWidth: Object.getOwnPropertyDescriptor(Element.prototype, "clientWidth"),
};
let observers: (() => void)[] = [];
let wrapWidth = 350;

beforeEach(() => {
  wrapWidth = 350;
  observers = [];
  // 100 px per tab, 60 px for More; the wrapper is `wrapWidth` wide
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get(this: HTMLElement) {
      if (this.hasAttribute("data-measure-more")) return 60;
      if (this.hasAttribute("data-measure-tab")) return 100;
      return 0;
    },
  });
  Object.defineProperty(Element.prototype, "clientWidth", {
    configurable: true,
    get(this: Element) {
      return this.classList.contains("relative") && this.classList.contains("items-end") ? wrapWidth : 0;
    },
  });
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(cb: () => void) {
        observers.push(cb);
      }
      observe() {}
      disconnect() {}
    },
  );
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  if (original.offsetWidth) Object.defineProperty(HTMLElement.prototype, "offsetWidth", original.offsetWidth);
  else Reflect.deleteProperty(HTMLElement.prototype, "offsetWidth");
  if (original.clientWidth) Object.defineProperty(Element.prototype, "clientWidth", original.clientWidth);
  else Reflect.deleteProperty(Element.prototype, "clientWidth");
});

const tabNames = () => within(screen.getByRole("tablist")).getAllByRole("tab").map((t) => t.textContent);

describe("Tabs overflow menu", () => {
  it("shows what fits and a More menu button for the rest", () => {
    render(<Harness />);
    expect(tabNames()).toEqual(["Tab 0", "Tab 1"]);
    const more = screen.getByRole("button", { name: "More agent tabs" });
    expect(more).toHaveAttribute("aria-haspopup", "menu");
    expect(more).toHaveAttribute("aria-expanded", "false");
  });

  it("has no More button when every tab fits, and re-measures on resize", () => {
    wrapWidth = 900;
    render(<Harness />);
    expect(tabNames()).toHaveLength(8);
    expect(screen.queryByRole("button", { name: /More/ })).toBeNull();
    wrapWidth = 350;
    act(() => observers.forEach((cb) => cb()));
    expect(tabNames()).toEqual(["Tab 0", "Tab 1"]);
    expect(screen.getByRole("button", { name: "More agent tabs" })).toBeInTheDocument();
  });

  it("opens with Enter, lists the hidden tabs and selecting one shows it in the strip", async () => {
    render(<Harness />);
    const more = screen.getByRole("button", { name: "More agent tabs" });
    more.focus();
    fireEvent.keyDown(more, { key: "Enter" });
    expect(more).toHaveAttribute("aria-expanded", "true");
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
      "Tab 2", "Tab 3", "Tab 4", "Tab 5", "Tab 6", "Tab 7",
    ]);
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Tab 6" }));
    await waitFor(() => expect(screen.getByTestId("current")).toHaveTextContent("t6"));
    // the selected tab is visible (it replaced the last visible one) and is the selected tab
    expect(tabNames()).toEqual(["Tab 0", "Tab 6"]);
    expect(screen.getByRole("tab", { name: "Tab 6" })).toHaveAttribute("aria-selected", "true");
  });

  it("closes with Escape and returns focus to the button", async () => {
    render(<Harness />);
    const more = screen.getByRole("button", { name: "More agent tabs" });
    more.focus();
    fireEvent.keyDown(more, { key: "Enter" });
    const menu = await screen.findByRole("menu");
    fireEvent.keyDown(menu, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(more).toHaveAttribute("aria-expanded", "false");
  });

  it("arrow keys move across every tab, including ones in the overflow, and keep the focus on the selected tab", async () => {
    render(<Harness />);
    const first = screen.getByRole("tab", { name: "Tab 0" });
    first.focus();
    fireEvent.keyDown(first, { key: "End" });
    await waitFor(() => expect(screen.getByTestId("current")).toHaveTextContent("t7"));
    const last = await screen.findByRole("tab", { name: "Tab 7" });
    expect(last).toHaveAttribute("aria-selected", "true");
    await waitFor(() => expect(last).toHaveFocus());
    expect(tabNames()).toEqual(["Tab 0", "Tab 7"]);
    fireEvent.keyDown(last, { key: "ArrowRight" }); // wraps to the first tab
    await waitFor(() => expect(screen.getByTestId("current")).toHaveTextContent("t0"));
    await waitFor(() => expect(screen.getByRole("tab", { name: "Tab 0" })).toHaveFocus());
  });

  it("keeps a roving tabindex: only the selected tab is in the tab order", () => {
    render(<Harness initial="t1" />);
    const tabs = within(screen.getByRole("tablist")).getAllByRole("tab");
    expect(tabs.map((t) => t.getAttribute("tabindex"))).toEqual(["-1", "0"]);
  });

  it("the off-screen measuring copy is hidden from assistive technology and not focusable", () => {
    const { container } = render(<Harness />);
    const copy = container.querySelector("[data-measure-tab]")!.parentElement!;
    expect(copy).toHaveAttribute("aria-hidden", "true");
    expect(copy).toHaveAttribute("inert");
    expect(copy.querySelectorAll("button, a, [tabindex]")).toHaveLength(0);
    // and it does not duplicate the tab ids or roles
    expect(screen.getAllByRole("tab")).toHaveLength(2);
  });
});
