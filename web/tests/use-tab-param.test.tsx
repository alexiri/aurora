import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useTabParam } from "@/lib/use-tab-param";

// P5-REFINE-13 (owner's "Try it in chat" bug): a navigation to the same agent that arrives with no `?tab=` leaves the
// page the person was on (Access) because the tab is state. The hook now follows the router back to the default page.

let search = "tab=access";
vi.mock("next/navigation", () => ({ useSearchParams: () => new URLSearchParams(search) }));

const PAGES = ["chat", "instructions", "access"] as const;

beforeEach(() => {
  search = "tab=access";
  window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=access");
});

describe("useTabParam", () => {
  it("reads ?tab= on load", () => {
    const { result } = renderHook(() => useTabParam(PAGES, "chat"));
    expect(result.current[0]).toBe("access");
  });

  it("a navigation that arrives with no tab resets to the default page", () => {
    const { result, rerender } = renderHook(() => useTabParam(PAGES, "chat"));
    expect(result.current[0]).toBe("access");
    search = "draft=hi";
    window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?draft=hi");
    rerender();
    expect(result.current[0]).toBe("chat");
  });

  it("uses the default of `initial` when the hook has one", () => {
    const { result, rerender } = renderHook(() => useTabParam(PAGES, "chat", { initial: () => "instructions" }));
    search = "";
    window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel");
    rerender();
    expect(result.current[0]).toBe("instructions");
  });

  it("a person's own tab click (history.replaceState with tab=) is not a reset", () => {
    const { result, rerender } = renderHook(() => useTabParam(PAGES, "chat"));
    act(() => result.current[1]("instructions"));
    search = "tab=instructions";
    rerender();
    expect(result.current[0]).toBe("instructions");
  });

  it("adopts a new ?tab= from the router", () => {
    const { result, rerender } = renderHook(() => useTabParam(PAGES, "chat"));
    search = "tab=chat";
    // window.location is still the previous page's (no tab) while the router has the link's
    window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel");
    rerender();
    expect(result.current[0]).toBe("chat");
  });
});
