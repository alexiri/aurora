import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { SavedNotice } from "@/components/ui/notice";
import { TypeToConfirmDelete } from "@/components/ui/type-to-confirm-delete";
import { ApiError } from "@/lib/api/client";

describe("TypeToConfirmDelete", () => {
  it("enables the button only for the exact name, then removes with what was typed", async () => {
    const remove = vi.fn(async () => {});
    const onDeleted = vi.fn();
    render(<TypeToConfirmDelete id="del" fieldLabel="URL name" slug="acme" buttonLabel="Delete it" remove={remove} onDeleted={onDeleted} />);
    const button = screen.getByRole("button", { name: "Delete it" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("URL name"), { target: { value: "acm" } });
    expect(button.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("URL name"), { target: { value: "acme" } });
    expect(button.disabled).toBe(false);
    fireEvent.click(button);
    await waitFor(() => expect(onDeleted).toHaveBeenCalled());
    expect(remove).toHaveBeenCalledWith("acme");
  });

  it("shows a failure and lets the person try again", async () => {
    const remove = vi.fn(async () => {
      throw new ApiError("unmapped", "Not today.", null, 409);
    });
    const onDeleted = vi.fn();
    render(<TypeToConfirmDelete id="del" fieldLabel="URL name" slug="acme" buttonLabel="Delete it" remove={remove} onDeleted={onDeleted} />);
    fireEvent.change(screen.getByLabelText("URL name"), { target: { value: "acme" } });
    fireEvent.click(screen.getByRole("button", { name: "Delete it" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Not today.");
    expect(onDeleted).not.toHaveBeenCalled();
    expect((screen.getByRole("button", { name: "Delete it" }) as HTMLButtonElement).disabled).toBe(false);
  });
});

describe("SavedNotice", () => {
  it("shows its words as a status while shown", () => {
    const { rerender, container } = render(<SavedNotice show={false}>Settings saved.</SavedNotice>);
    expect(container.innerHTML).toBe("");
    rerender(<SavedNotice show>Settings saved.</SavedNotice>);
    expect(screen.getByRole("status")).toHaveTextContent("Settings saved.");
  });
});
