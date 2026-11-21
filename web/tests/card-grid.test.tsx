import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { CardGrid } from "@/components/ui/card-grid";

describe("CardGrid (D-439)", () => {
  it("is a labelled list that steps 1, 2, 3, 4 columns with the width", () => {
    render(
      <CardGrid aria-label="Things">
        <li>One</li>
        <li>Two</li>
      </CardGrid>,
    );
    const list = screen.getByRole("list", { name: "Things" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(2);
    for (const cls of ["grid", "gap-4", "@2xl:grid-cols-2", "@4xl:grid-cols-3", "@7xl:grid-cols-4"]) {
      expect(list).toHaveClass(cls);
    }
  });

  it("passes a class name through", () => {
    render(<CardGrid aria-label="Wide" className="mt-2"><li>One</li></CardGrid>);
    expect(screen.getByRole("list", { name: "Wide" })).toHaveClass("mt-2", "grid");
  });
});
