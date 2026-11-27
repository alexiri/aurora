import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import { SETTINGS_NARROW } from "@/components/agents/settings-shell";
import { SubTabs } from "@/components/ui/sub-tabs";
import { NARROW_SCREEN, ScreenChromeContext, useNarrowScreen } from "@/components/ui/list-detail";
import { tabId, tabPanelId } from "@/components/ui/tabs";

// (D-507) The quiet second row under a tab: one scrolling line with a fade where there is more, the selected pill
// brought into view by scrolling only the row. jsdom has no layout, so the boxes are given.
const items = ["Files", "Terminal", "Connect"].map((l) => ({ value: l.toLowerCase(), label: l }));

function Row({ initial = "files" }: { initial?: string }) {
  const [value, setValue] = useState(initial);
  return <SubTabs items={items} value={value} onValueChange={setValue} label="Computer pages" idBase="agent" />;
}

/** A 200 px row of three 100 px pills; `scrollLeft` moves them. */
function layout() {
  const rect = (left: number, width: number) => ({ left, right: left + width, width, top: 0, bottom: 44, height: 44, x: left, y: 0, toJSON: () => ({}) }) as DOMRect;
  const rects = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    if (this.getAttribute("role") === "tablist") return rect(0, 200);
    const list = this.closest<HTMLElement>('[role="tablist"]');
    if (!list) return rect(0, 0);
    return rect(Array.from(list.children).indexOf(this) * 100 - list.scrollLeft, 100);
  });
  const client = vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(200);
  const scroll = vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(300);
  return () => [rects, client, scroll].forEach((s) => s.mockRestore());
}

describe("SubTabs", () => {
  it("is a tablist of pills: the selected one is the current page, arrows move, and it never collapses into a menu", () => {
    render(<Row />);
    const list = screen.getByRole("tablist", { name: "Computer pages" });
    const files = screen.getByRole("tab", { name: "Files" });
    expect(files).toHaveAttribute("aria-selected", "true");
    expect(files).toHaveAttribute("aria-current", "page");
    expect(files).toHaveAttribute("id", tabId("agent", "files"));
    expect(files).toHaveAttribute("aria-controls", tabPanelId("agent", "files"));
    expect(files.firstElementChild!.className).toContain("rounded-full");
    expect(list.className).toContain("overflow-x-auto");
    expect(list.className).not.toContain("flex-wrap");
    fireEvent.keyDown(list, { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "Terminal" })).toHaveAttribute("aria-current", "page");
    expect(files).not.toHaveAttribute("aria-current");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("fades the edge that has more, and scrolls only the row to bring the selected pill into view", () => {
    const restore = layout();
    try {
      render(<Row initial="connect" />);
      const list = screen.getByRole("tablist");
      // Connect lies at 200–300 in a 200 px row: the row scrolls by what is hidden plus the margin for the fade.
      expect(list.scrollLeft).toBe(132);
      expect(screen.getByTestId("sub-tabs-fade-start")).toHaveAttribute("aria-hidden", "true");
      list.scrollLeft = 0;
      fireEvent.scroll(list);
      expect(screen.queryByTestId("sub-tabs-fade-start")).toBeNull();
      expect(screen.getByTestId("sub-tabs-fade-end")).toBeInTheDocument();
      list.scrollLeft = 100;
      fireEvent.scroll(list);
      expect(screen.queryByTestId("sub-tabs-fade-end")).toBeNull();
    } finally {
      restore();
    }
  });

  it("has no fade when everything fits", () => {
    render(<Row />);
    expect(screen.queryByTestId("sub-tabs-fade-start")).toBeNull();
    expect(screen.queryByTestId("sub-tabs-fade-end")).toBeNull();
  });
});

describe("the list-and-item cut-off (D-507)", () => {
  function Probe() {
    return <span>{useNarrowScreen() ? "two screens" : "two columns"}</span>;
  }
  const at = (width: number) => {
    const original = window.matchMedia;
    window.matchMedia = ((query: string) => ({
      matches: width <= Number(/max-width: (\d+)px/.exec(query)?.[1] ?? 0),
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    })) as unknown as typeof window.matchMedia;
    return () => {
      window.matchMedia = original;
    };
  };

  it("is 1024 px on an agent's Settings page, where there is no second column, and unchanged elsewhere", () => {
    expect(SETTINGS_NARROW).toBe("(max-width: 1023px)");
    expect(NARROW_SCREEN).toBe("(max-width: 1099px)");
    const restore = at(1024);
    try {
      const { rerender } = render(<Probe />);
      expect(screen.getByText("two screens")).toBeInTheDocument();
      rerender(
        <ScreenChromeContext.Provider value={{ actionSlot: null, setSubScreen: () => undefined, narrowQuery: SETTINGS_NARROW }}>
          <Probe />
        </ScreenChromeContext.Provider>,
      );
      expect(screen.getByText("two columns")).toBeInTheDocument();
    } finally {
      restore();
    }
  });
});
