import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { LoadMore } from "@/components/ui/load-more";

describe("LoadMore", () => {
  it("fetches the next page, and is disabled while one loads", () => {
    const fetchNextPage = vi.fn();
    const { rerender } = render(<LoadMore query={{ hasNextPage: true, isFetchingNextPage: false, fetchNextPage }} />);
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    expect(fetchNextPage).toHaveBeenCalledTimes(1);
    rerender(<LoadMore query={{ hasNextPage: true, isFetchingNextPage: true, fetchNextPage }} label="Load more runs" />);
    expect((screen.getByRole("button", { name: "Load more runs" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("renders nothing on the last page", () => {
    const { container } = render(<LoadMore query={{ hasNextPage: false, isFetchingNextPage: false, fetchNextPage: vi.fn() }} />);
    expect(container.innerHTML).toBe("");
  });
});
