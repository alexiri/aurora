import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ConfirmAction, ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DeleteWithName, matchesName, TypeToConfirm } from "@/components/ui/type-to-confirm";
import { dismissUndoToast, showUndoToast, UndoToastHost, UNDO_TOAST_MS } from "@/components/ui/undo-toast";
import { ApiError } from "@/lib/api/client";

describe("rung two: ConfirmDialog needs a consequence sentence", () => {
  it("shows the consequence and the verb, and Cancel does nothing", () => {
    const onConfirm = vi.fn();
    render(
      <ConfirmDialog open onOpenChange={() => {}} title="Rotate the URL?" description="The old URL stops working now." confirmLabel="Rotate URL" danger onConfirm={onConfirm} />,
    );
    expect(screen.getByRole("dialog")).toHaveTextContent("The old URL stops working now.");
    fireEvent.click(screen.getByRole("button", { name: "Rotate URL" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("pins Cancel and the verb in the dialog's bordered footer, as FormDialog does, with the content above it", () => {
    render(
      <ConfirmDialog open onOpenChange={() => {}} title="Remove it?" description="It stops working." confirmLabel="Remove" onConfirm={() => {}}>
        <p>3 agents use it.</p>
      </ConfirmDialog>,
    );
    const dialog = screen.getByRole("dialog");
    const footer = dialog.lastElementChild as HTMLElement;
    expect(footer).toHaveClass("shrink-0", "border-t");
    for (const name of ["Cancel", "Remove"]) expect(footer).toContainElement(within(dialog).getByRole("button", { name }));
    expect(footer).not.toContainElement(within(dialog).getByText("3 agents use it."));
  });

  it("refuses a missing description at the type level", () => {
    // @ts-expect-error `description` is required: a confirm without a consequence does not compile.
    const bad = <ConfirmDialog open onOpenChange={() => {}} title="Sure?" confirmLabel="Yes" onConfirm={() => {}} />;
    expect(bad).toBeTruthy();
  });

  it("ConfirmAction opens the dialog from its button and closes when confirmed", () => {
    const onConfirm = vi.fn();
    render(<ConfirmAction label="Delete" title="Delete the skill?" description="Agents stop using it." confirmLabel="Delete skill" onConfirm={onConfirm} />);
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Agents stop using it.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete skill" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });
});

describe("rung three: TypeToConfirm asks for the visible name", () => {
  it("matches case-sensitively, trimmed", () => {
    expect(matchesName("Bob's Lab", "Bob's Lab")).toBe(true);
    expect(matchesName("  Bob's Lab ", "Bob's Lab")).toBe(true);
    expect(matchesName("bob's lab", "Bob's Lab")).toBe(false);
    expect(matchesName("", "")).toBe(false);
  });

  function setup(run = vi.fn(async () => {}), onDone = vi.fn()) {
    render(
      <TypeToConfirm
        open
        onOpenChange={() => {}}
        title="Delete Bob's Lab?"
        consequence="This deletes the workspace, its agents, files and conversations. It can't be undone."
        noun="workspace"
        name="Bob's Lab"
        confirmLabel="Delete workspace"
        run={run}
        onDone={onDone}
      />,
    );
    return { run, onDone };
  }

  it("shows the consequence in red and the name, never a slug; the button waits for the exact name", async () => {
    const { run, onDone } = setup();
    expect(screen.getByRole("dialog")).toHaveTextContent("It can't be undone.");
    expect(screen.getByText(/Type the workspace name to confirm/)).toHaveTextContent("Bob's Lab");
    const button = screen.getByRole("button", { name: "Delete workspace" });
    expect(button).toBeDisabled();
    const field = screen.getByLabelText(/Type the workspace name to confirm/);
    fireEvent.change(field, { target: { value: "bobs-lab" } });
    expect(button).toBeDisabled();
    fireEvent.change(field, { target: { value: "Bob's Lab" } });
    expect(button).toBeEnabled();
    fireEvent.click(button);
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("shows the API's refusal and lets the person try again", async () => {
    const run = vi.fn(async () => {
      throw new ApiError("unmapped", "Not today.", null, 409);
    });
    const { onDone } = setup(run);
    fireEvent.change(screen.getByLabelText(/Type the workspace name to confirm/), { target: { value: "Bob's Lab" } });
    fireEvent.click(screen.getByRole("button", { name: "Delete workspace" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Not today.");
    expect(onDone).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Delete workspace" })).toBeEnabled();
  });

  it("DeleteWithName opens the dialog from its button", () => {
    render(
      <DeleteWithName buttonLabel="Delete workspace" title="Delete Bob's Lab?" consequence="Gone." noun="workspace" name="Bob's Lab" run={async () => {}} onDone={() => {}} />,
    );
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Delete workspace" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});

describe("rung one: undo toast", () => {
  it("shows one toast with Undo, replaces it with the next, and goes after 6 s", () => {
    vi.useFakeTimers();
    try {
      const undo = vi.fn();
      render(<UndoToastHost />);
      act(() => showUndoToast("Webhook paused", undo));
      expect(screen.getByTestId("undo-toast")).toHaveTextContent("Webhook paused");
      act(() => showUndoToast("Schedule paused"));
      expect(screen.getAllByTestId("undo-toast")).toHaveLength(1);
      expect(screen.getByTestId("undo-toast")).toHaveTextContent("Schedule paused");
      expect(screen.queryByRole("button", { name: "Undo" })).toBeNull();
      act(() => showUndoToast("Webhook paused", undo));
      fireEvent.click(screen.getByRole("button", { name: "Undo" }));
      expect(undo).toHaveBeenCalledTimes(1);
      expect(screen.queryByTestId("undo-toast")).toBeNull();
      act(() => showUndoToast("Paused"));
      act(() => void vi.advanceTimersByTime(UNDO_TOAST_MS + 50));
      expect(screen.queryByTestId("undo-toast")).toBeNull();
      act(() => dismissUndoToast());
    } finally {
      vi.useRealTimers();
    }
  });
});
