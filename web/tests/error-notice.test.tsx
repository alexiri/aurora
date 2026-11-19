import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ErrorNotice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";

describe("ErrorNotice", () => {
  it("shows the error's words as an alert", () => {
    render(<ErrorNotice error={new ApiError("some_unmapped_code", "Only an owner can do this.", null, 403)} className="mt-3" />);
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("Only an owner can do this.");
    expect(alert.className).toContain("mt-3");
  });

  it("uses a screen's own formatter when given one", () => {
    render(<ErrorNotice error={new Error("x")} format={() => "custom words"} />);
    expect(screen.getByRole("alert").textContent).toBe("custom words");
  });

  it("renders nothing without an error", () => {
    const { container } = render(<ErrorNotice error={null} />);
    expect(container.innerHTML).toBe("");
  });
});
