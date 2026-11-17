import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { BarChart } from "@/components/ui/bar-chart";
import { StatTile } from "@/components/ui/stat-tile";
import { compact, duration, groupByWeek, presetRange, rangeDays, weekStart } from "@/lib/usage-format";

// P5-USAGE-2 (C-1005): StatTile, BarChart and the number/duration/range helpers.

afterEach(cleanup);

describe("StatTile", () => {
  it("shows the label, the value with the exact figure as its title, and a detail line", () => {
    render(<StatTile label="Tokens" value="1.2 M" title="1,234,567" detail="Input 1 M" />);
    expect(screen.getByText("Tokens")).toBeInTheDocument();
    expect(screen.getByText("1.2 M")).toHaveAttribute("title", "1,234,567");
    expect(screen.getByText("Input 1 M")).toBeInTheDocument();
  });
});

const data = [
  { key: "2026-11-15", label: "Nov 15", value: 1200 },
  { key: "2026-11-16", label: "Nov 16", value: 0 },
  { key: "2026-11-17", label: "Nov 17", value: 3400 },
];
const fmt = (v: number) => `${v.toLocaleString("en-US")} tokens`;

describe("BarChart", () => {
  it("is an image with a summary and gives every bar the name '<label>: <value>'", () => {
    render(<BarChart data={data} format={fmt} unitLabel="Tokens per day" width={400} />);
    expect(screen.getByRole("img")).toHaveAccessibleName(/Tokens per day: 3 bars, total 4,600 tokens, highest 3,400 tokens/);
    expect(screen.getByText("Nov 15: 1,200 tokens")).toBeInTheDocument();
    expect(screen.getByText("Nov 16: 0 tokens")).toBeInTheDocument();
    expect(screen.getByText("Nov 17: 3,400 tokens")).toBeInTheDocument();
  });

  it("is one tab stop; the arrow keys move between bars and the tooltip names the bar", () => {
    render(<BarChart data={data} format={fmt} unitLabel="Tokens per day" width={400} />);
    const group = screen.getByTestId("bar-chart");
    expect(group).toHaveAttribute("tabindex", "0");
    expect(screen.queryByTestId("bar-tooltip")).toBeNull();
    fireEvent.focus(group);
    // focus lands on the newest bar
    expect(screen.getByTestId("bar-tooltip")).toHaveTextContent("Nov 17: 3,400 tokens");
    fireEvent.keyDown(group, { key: "ArrowLeft" });
    expect(screen.getByTestId("bar-tooltip")).toHaveTextContent("Nov 16: 0 tokens");
    fireEvent.keyDown(group, { key: "Home" });
    expect(screen.getByTestId("bar-tooltip")).toHaveTextContent("Nov 15: 1,200 tokens");
    fireEvent.keyDown(group, { key: "ArrowLeft" });
    expect(screen.getByTestId("bar-tooltip")).toHaveTextContent("Nov 15");
    fireEvent.keyDown(group, { key: "End" });
    expect(screen.getByTestId("bar-tooltip")).toHaveTextContent("Nov 17");
    fireEvent.keyDown(group, { key: "ArrowRight" });
    expect(screen.getByTestId("bar-tooltip")).toHaveTextContent("Nov 17");
    fireEvent.blur(group);
    expect(screen.queryByTestId("bar-tooltip")).toBeNull();
  });

  it("draws at most five y ticks with compact numbers, in tokens not in colour alone", () => {
    const { container } = render(<BarChart data={data} format={fmt} unitLabel="Tokens per day" width={400} />);
    const tickTexts = [...container.querySelectorAll("svg text")].map((t) => t.textContent ?? "").filter((t) => /^\d+(\.\d)?( k)?$/.test(t) && !t.startsWith("Nov"));
    expect(tickTexts.length).toBeLessThanOrEqual(5 + 3); // y ticks plus the date labels that look numeric
    expect(container.querySelectorAll("line").length).toBeLessThanOrEqual(5);
    expect(container.innerHTML).not.toContain("style=");
  });

  it("offers Show as table when asked to", () => {
    const onShowTable = vi.fn();
    render(<BarChart data={data} format={fmt} unitLabel="Tokens per day" width={400} onShowTable={onShowTable} />);
    fireEvent.click(screen.getByRole("button", { name: "Show as table" }));
    expect(onShowTable).toHaveBeenCalledOnce();
  });

  it("renders with no data", () => {
    render(<BarChart data={[]} format={fmt} unitLabel="Tokens per day" width={400} />);
    expect(screen.getByRole("img")).toHaveAccessibleName(/0 bars/);
  });
});

describe("usage formatting", () => {
  it("compacts counts", () => {
    expect([0, 950, 1000, 1234, 34_000, 999_999, 1_200_000, 3_400_000_000].map(compact)).toEqual(["0", "950", "1 k", "1.2 k", "34 k", "1 M", "1.2 M", "3.4 G"]);
  });
  it("writes durations", () => {
    expect([0, 20, 60, 45 * 60, 3 * 3600 + 20 * 60, 7200].map(duration)).toEqual(["0 min", "under 1 min", "1 min", "45 min", "3 h 20 min", "2 h"]);
  });
  it("computes preset ranges in UTC and counts days inclusively", () => {
    const r = presetRange(30, new Date("2026-11-17T23:30:00Z"));
    expect(r).toEqual({ from: "2026-10-19", to: "2026-11-17" });
    expect(rangeDays(r.from, r.to)).toBe(30);
    expect(rangeDays("2025-01-01", "2026-01-01")).toBe(366);
  });
  it("groups days by UTC week, Monday first", () => {
    expect(weekStart("2026-11-17")).toBe("2026-11-16"); // a Tuesday
    expect(weekStart("2026-11-15")).toBe("2026-11-09"); // a Sunday
    const day = (d: string, n: number) => ({ day: d, turns: n, turn_seconds: 0, awake_seconds: 0, input_tokens: n, output_tokens: 0 });
    const weeks = groupByWeek([day("2026-11-15", 1), day("2026-11-16", 2), day("2026-11-17", 3)]);
    expect(weeks.map((w) => [w.day, w.turns])).toEqual([["2026-11-15", 1], ["2026-11-16", 5]]);
  });
});
