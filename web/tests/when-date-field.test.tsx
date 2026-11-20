import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DateField, isoToLocaleText, localeDatePattern, localeTextToIso } from "@/components/ui/date-field";
import { When, WHEN_TICK_MS } from "@/components/ui/when";

describe("When", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("renders <time> with the exact time as its title, and ticks from 'Just now' to '1 min ago' with no interaction", () => {
    vi.setSystemTime(new Date("2026-11-19T12:00:00Z"));
    const at = new Date("2026-11-19T11:59:45Z").toISOString();
    render(<When value={at} />);
    const el = screen.getByText("Just now");
    expect(el.tagName).toBe("TIME");
    expect(el).toHaveAttribute("dateTime", at);
    expect(el.getAttribute("title")).toMatch(/2026/);
    act(() => void vi.advanceTimersByTime(WHEN_TICK_MS));
    expect(screen.getByText("1 min ago")).toBeInTheDocument();
    act(() => void vi.advanceTimersByTime(WHEN_TICK_MS * 4));
    expect(screen.getByText("3 min ago")).toBeInTheDocument();
  });

  it("uses one timer for every row on the page", () => {
    const spy = vi.spyOn(globalThis, "setInterval");
    render(
      <>
        <When value="2026-11-19T11:00:00Z" />
        <When value="2026-11-19T10:00:00Z" />
        <When value="2026-11-19T09:00:00Z" />
      </>,
    );
    expect(spy.mock.calls.filter(([, ms]) => ms === WHEN_TICK_MS)).toHaveLength(1);
    spy.mockRestore();
  });

  it("pauses while the tab is hidden", () => {
    vi.setSystemTime(new Date("2026-11-19T12:00:00Z"));
    render(<When value={new Date("2026-11-19T11:59:45Z").toISOString()} />);
    Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
    act(() => void vi.advanceTimersByTime(WHEN_TICK_MS * 2));
    expect(screen.getByText("Just now")).toBeInTheDocument();
    Object.defineProperty(document, "visibilityState", { value: "visible", configurable: true });
    act(() => void document.dispatchEvent(new Event("visibilitychange")));
    expect(screen.queryByText("Just now")).toBeNull();
  });

  it("shows the empty word for a missing time", () => {
    render(<When value={null} empty="Never" />);
    expect(screen.getByText("Never")).toBeInTheDocument();
  });
});

describe("DateField", () => {
  it("converts both ways in the browser's locale order", () => {
    const { order } = localeDatePattern("en-GB");
    expect(order).toEqual(["day", "month", "year"]);
    expect(localeDatePattern("en-US").order).toEqual(["month", "day", "year"]);
    expect(isoToLocaleText("2026-11-19", "en-GB")).toBe("19/11/2026");
    expect(isoToLocaleText("2026-11-19", "en-US")).toBe("11/19/2026");
    expect(localeTextToIso("19/11/2026", "en-GB")).toBe("2026-11-19");
    expect(localeTextToIso("11/19/2026", "en-US")).toBe("2026-11-19");
    expect(localeTextToIso("31/02/2026", "en-GB")).toBeNull();
    expect(localeTextToIso("nope", "en-GB")).toBeNull();
  });

  it("sends ISO YYYY-MM-DD and shows the locale text", () => {
    const onChange = vi.fn();
    const first = localeDatePattern().order[0];
    render(<DateField aria-label="From date" value="" onChange={onChange} />);
    const input = screen.getByLabelText("From date");
    const text = first === "day" ? "19/11/2026" : first === "year" ? "2026/11/19" : "11/19/2026";
    fireEvent.change(input, { target: { value: text } });
    expect(onChange).toHaveBeenCalledWith("2026-11-19");
    fireEvent.change(input, { target: { value: "" } });
    expect(onChange).toHaveBeenLastCalledWith("");
  });

  it("marks an impossible date invalid and sends nothing", () => {
    const onChange = vi.fn();
    render(<DateField aria-label="To date" value="" onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("To date"), { target: { value: "99/99/2026" } });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByLabelText("To date")).toHaveAttribute("aria-invalid", "true");
  });
});
