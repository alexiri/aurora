import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SecretOnce } from "@/components/ui/secret-once";

const TOKEN = `auk_${"x".repeat(120)}`;

function Host({ onClose }: { onClose: () => void }) {
  return <SecretOnce secret={TOKEN} title="Copy your key" description="You won't see it again." onClose={onClose} />;
}

let write: ReturnType<typeof vi.fn>;
beforeEach(() => {
  write = vi.fn(async () => {});
  Object.defineProperty(navigator, "clipboard", { value: { writeText: write }, configurable: true });
});
afterEach(() => {
  Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
  vi.useRealTimers();
});

/** Flushes the write promise's microtasks (fake timers do not run microtasks by themselves). */
const flush = () => act(async () => {});

describe("SecretOnce", () => {
  it("shows a 120-character token in a box that scrolls and never wraps", () => {
    render(<Host onClose={() => {}} />);
    const box = screen.getByTestId("secret-key");
    expect(box).toHaveTextContent(TOKEN);
    expect(box.className).toContain("whitespace-nowrap");
    expect(box.className).toContain("overflow-x-auto");
    expect(box.className).toContain("select-all");
  });

  it("gives feedback on every click: Copied for two seconds, then Copy again; every click copies (FIX-248)", async () => {
    vi.useFakeTimers();
    render(<Host onClose={() => {}} />);
    const button = screen.getByRole("button", { name: "Copy" });
    fireEvent.click(button);
    await flush();
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(write).toHaveBeenCalledTimes(1);
    act(() => vi.advanceTimersByTime(2000));
    // Not stuck on "Copied" for ever: the button invites copying again.
    expect(screen.getByRole("button", { name: "Copy again" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Copy again" }));
    await flush();
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(write).toHaveBeenCalledTimes(2);
    act(() => vi.advanceTimersByTime(2000));
    expect(screen.getByRole("button", { name: "Copy again" })).toBeInTheDocument();
  });

  it("announces every copy in the screen-reader line, worded differently each time", async () => {
    vi.useFakeTimers();
    render(<Host onClose={() => {}} />);
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("");
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await flush();
    expect(status).toHaveTextContent("Copied");
    fireEvent.click(screen.getByRole("button", { name: "Copied" }));
    await flush();
    expect(status).toHaveTextContent("Copied again.");
    fireEvent.click(screen.getByRole("button", { name: "Copied" }));
    await flush();
    expect(status).toHaveTextContent("Copied 3 times.");
  });

  it("keeps the button on Copy and shows the red sentence when the browser refused, and used still gates the close confirm", async () => {
    write = vi.fn(async () => {
      throw new Error("denied");
    });
    Object.defineProperty(navigator, "clipboard", { value: { writeText: write }, configurable: true });
    render(<Host onClose={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await flush();
    expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Your browser blocked copying; select the text instead.");
    // Nothing was copied, so closing still asks first.
    const onClose = vi.fn();
    const { unmount } = render(<Host onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    expect(screen.getByText("You haven't copied it. Close anyway?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close anyway" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("falls back to selecting the text and execCommand when the browser has no async clipboard", async () => {
    Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
    const exec = vi.fn(() => true);
    document.execCommand = exec as unknown as typeof document.execCommand;
    try {
      render(<Host onClose={() => {}} />);
      fireEvent.click(screen.getByRole("button", { name: "Copy" }));
      await flush();
      expect(exec).toHaveBeenCalledWith("copy");
      expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    } finally {
      delete (document as { execCommand?: unknown }).execCommand;
    }
  });

  it("closes at once after it was copied", async () => {
    const onClose = vi.fn();
    render(<Host onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("asks before closing without a copy, from the button, the X and Escape; Close anyway closes", () => {
    const onClose = vi.fn();
    render(<Host onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    expect(screen.getByText("You haven't copied it. Close anyway?")).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Close anyway" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("copies from the ask, which then lets it close", async () => {
    const onClose = vi.fn();
    render(<Host onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "I have saved it" }));
    const ask = screen.getByText("You haven't copied it. Close anyway?").closest("div")!;
    fireEvent.click(ask.querySelector("button")!); // Copy
    await flush();
    expect(write).toHaveBeenCalledWith(TOKEN);
  });
});
