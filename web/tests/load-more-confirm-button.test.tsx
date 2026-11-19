import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ConfirmButton } from "@/components/ui/confirm-button";
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

describe("ConfirmButton", () => {
  it("asks first: Delete, then Confirm delete with Cancel; only the confirm acts", () => {
    const onConfirm = vi.fn();
    render(<ConfirmButton onConfirm={onConfirm} />);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("button", { name: "Delete" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("takes its own words and disables the confirm while pending", () => {
    render(<ConfirmButton label="Remove" confirmLabel="Confirm remove" pending onConfirm={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect((screen.getByRole("button", { name: "Confirm remove" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
