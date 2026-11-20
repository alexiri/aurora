import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { InlineSaved, SAVED_MS, SaveBar, useSaveBar } from "@/components/ui/save-bar";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

function Harness({ errorCount = 0 }: { errorCount?: number }) {
  const bar = useSaveBar();
  return (
    <>
      <button onClick={bar.flash}>flash</button>
      <SaveBar dirty={false} saved={bar.saved} onDiscard={() => {}} onUndo={() => {}} savedMessage="Saved. New agents use these defaults." errorCount={errorCount} />
    </>
  );
}

describe("SaveBar", () => {
  it("is hidden until the first change", () => {
    const { container } = render(<SaveBar dirty={false} onDiscard={() => {}} />);
    expect(container.innerHTML).toBe("");
  });

  it("shows Unsaved changes with Discard and Save, and sends both", () => {
    const onSave = vi.fn();
    const onDiscard = vi.fn();
    render(<SaveBar dirty onSave={onSave} onDiscard={onDiscard} />);
    const region = screen.getByRole("region", { name: "Unsaved changes" });
    expect(region).toHaveTextContent("Unsaved changes");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onDiscard).toHaveBeenCalledTimes(1);
  });

  it("is a submit button of the form without onSave", () => {
    render(<SaveBar dirty onDiscard={() => {}} />);
    expect(screen.getByRole("button", { name: "Save" })).toHaveAttribute("type", "submit");
  });

  it("says Saving… and disables both buttons while saving", () => {
    render(<SaveBar dirty saving onSave={() => {}} onDiscard={() => {}} />);
    expect(screen.getByRole("button", { name: "Saving…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Discard" })).toBeDisabled();
  });

  it("says how many fields need fixing, and the failure's own sentence otherwise (Save stays enabled)", () => {
    const { rerender } = render(<SaveBar dirty errorCount={2} onSave={() => {}} onDiscard={() => {}} />);
    expect(screen.getByRole("alert")).toHaveTextContent("2 fields need fixing");
    rerender(<SaveBar dirty errorCount={1} onSave={() => {}} onDiscard={() => {}} />);
    expect(screen.getByRole("alert")).toHaveTextContent("1 field needs fixing");
    rerender(<SaveBar dirty error="Someone changed these settings meanwhile." onSave={() => {}} onDiscard={() => {}} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Someone changed these settings meanwhile.");
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  it("scrolls to and focuses the first bad field after a failed save", () => {
    const scroll = vi.fn();
    (Element.prototype as unknown as { scrollIntoView: () => void }).scrollIntoView = scroll;
    const { rerender } = render(
      <>
        <input aria-label="ok" />
        <input aria-label="bad" aria-invalid="true" />
        <SaveBar dirty saving errorCount={0} onSave={() => {}} onDiscard={() => {}} />
      </>,
    );
    rerender(
      <>
        <input aria-label="ok" />
        <input aria-label="bad" aria-invalid="true" />
        <SaveBar dirty saving={false} errorCount={1} onSave={() => {}} onDiscard={() => {}} />
      </>,
    );
    expect(document.activeElement).toBe(screen.getByLabelText("bad"));
    expect(scroll).toHaveBeenCalled();
  });

  it("shows Saved with Undo for two seconds, then disappears", () => {
    const { container } = render(<Harness />);
    expect(container.querySelector("[data-testid=save-bar-saved]")).toBeNull();
    fireEvent.click(screen.getByText("flash"));
    expect(screen.getByRole("status")).toHaveTextContent("Saved. New agents use these defaults.");
    expect(screen.getByRole("button", { name: "Undo" })).toBeInTheDocument();
    act(() => void vi.advanceTimersByTime(SAVED_MS - 100));
    expect(screen.getByRole("status")).toBeInTheDocument();
    act(() => void vi.advanceTimersByTime(200));
    expect(container.querySelector("[data-testid=save-bar-saved]")).toBeNull();
  });
});

describe("InlineSaved", () => {
  it("shows a check and Saved only while shown, as a polite status", () => {
    const { rerender } = render(<InlineSaved show={false} />);
    expect(screen.getByRole("status")).toHaveTextContent("");
    rerender(<InlineSaved show />);
    expect(screen.getByRole("status")).toHaveTextContent("Saved");
    expect(screen.getByRole("status")).toHaveAttribute("aria-live", "polite");
  });
});
