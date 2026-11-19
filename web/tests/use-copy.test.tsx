import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { RequestReference } from "@/components/states";
import { ApiError } from "@/lib/api/client";
import { copyLabel, useCopy } from "@/lib/use-copy";

// P5-DEDUP-2 P5: one copy helper. "Copied" only once the browser accepted the write; a rejected write or a missing
// clipboard is shown as a failure, never an unhandled rejection or a TypeError.

function clipboard(writeText: (t: string) => Promise<void> | undefined) {
  vi.stubGlobal("navigator", { clipboard: { writeText } });
}

function Probe({ ms }: { ms?: number }) {
  const { state, copy } = useCopy(ms);
  return (
    <>
      <button onClick={() => copy("the text")}>go</button>
      <output>{copyLabel(state)}</output>
    </>
  );
}

describe("useCopy", () => {
  it("is 'Copied' only after the write resolves, then goes back to idle", async () => {
    let resolve!: () => void;
    const writeText = vi.fn(() => new Promise<void>((r) => (resolve = r)));
    clipboard(writeText);
    render(<Probe ms={30} />);
    fireEvent.click(screen.getByText("go"));
    expect(writeText).toHaveBeenCalledWith("the text");
    expect(screen.getByRole("status")).toHaveTextContent("Copy");
    expect(screen.getByRole("status")).not.toHaveTextContent("Copied");
    await act(async () => resolve());
    expect(screen.getByRole("status")).toHaveTextContent("Copied");
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent(/^Copy$/));
  });

  it("shows a failure, not 'Copied', when the write is rejected", async () => {
    clipboard(async () => {
      throw new Error("denied");
    });
    render(<Probe />);
    fireEvent.click(screen.getByText("go"));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Copy failed"));
  });

  it("shows a failure when there is no clipboard or it throws at once", () => {
    vi.stubGlobal("navigator", {});
    const { unmount } = render(<Probe />);
    fireEvent.click(screen.getByText("go"));
    expect(screen.getByRole("status")).toHaveTextContent("Copy failed");
    unmount();
    clipboard(() => {
      throw new Error("sync");
    });
    render(<Probe />);
    fireEvent.click(screen.getByText("go"));
    expect(screen.getByRole("status")).toHaveTextContent("Copy failed");
  });
});

describe("a copy button that had no failure handling (the request reference)", () => {
  const err = new ApiError("not_found", "no", null, 404, "0af7651916cd43dd8448eb211c80319c");

  it("says the copy failed when the clipboard rejects", async () => {
    clipboard(async () => {
      throw new Error("denied");
    });
    render(<RequestReference error={err} />);
    fireEvent.click(screen.getByRole("button", { name: /copy reference/i }));
    await waitFor(() => expect(screen.getByRole("button", { name: /copy reference/i })).toHaveTextContent("Copy failed"));
  });

  it("does not throw without a clipboard", () => {
    vi.stubGlobal("navigator", {});
    render(<RequestReference error={err} />);
    fireEvent.click(screen.getByRole("button", { name: /copy reference/i }));
    expect(screen.getByRole("button", { name: /copy reference/i })).toHaveTextContent("Copy failed");
  });
});
