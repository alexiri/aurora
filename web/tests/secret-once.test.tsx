import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
});

describe("SecretOnce", () => {
  it("shows a 120-character token in a box that scrolls and never wraps", () => {
    render(<Host onClose={() => {}} />);
    const box = screen.getByTestId("secret-key");
    expect(box).toHaveTextContent(TOKEN);
    expect(box.className).toContain("whitespace-nowrap");
    expect(box.className).toContain("overflow-x-auto");
    expect(box.className).toContain("select-all");
  });

  it("turns Copy into Copied, which stays once used", async () => {
    render(<Host onClose={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument());
    expect(write).toHaveBeenCalledWith(TOKEN);
    await new Promise((r) => setTimeout(r, 2200));
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("closes at once after it was copied", async () => {
    const onClose = vi.fn();
    render(<Host onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => screen.getByRole("button", { name: "Copied" }));
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
    await waitFor(() => expect(write).toHaveBeenCalledWith(TOKEN));
  });
});
