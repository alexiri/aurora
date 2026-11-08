import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { Meter } from "@/components/ui/meter";

afterEach(cleanup);

describe("Meter", () => {
  it("draws the fill as an SVG width attribute, with no inline style", () => {
    const { container } = render(<Meter role="meter" aria-label="Body" aria-valuenow={25} aria-valuemin={0} aria-valuemax={100} ratio={0.25} tone="danger" />);
    expect(screen.getByRole("meter", { name: "Body" })).toHaveAttribute("aria-valuenow", "25");
    const rect = container.querySelector("rect")!;
    expect(rect.getAttribute("width")).toBe("25");
    expect(rect.getAttribute("class")).toContain("fill-danger-ink");
    expect(container.querySelector("[style]")).toBeNull();
  });

  it("clamps the ratio", () => {
    const { container, rerender } = render(<Meter ratio={3} />);
    expect(container.querySelector("rect")!.getAttribute("width")).toBe("100");
    rerender(<Meter ratio={-1} />);
    expect(container.querySelector("rect")!.getAttribute("width")).toBe("0");
    rerender(<Meter ratio={Number.NaN} />);
    expect(container.querySelector("rect")!.getAttribute("width")).toBe("0");
  });
});
