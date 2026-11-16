import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { CopyValue } from "@/components/ui/copy-value";

// P5-UX-13 (C-942): the shared CopyValue primitive: label, value, Copy; "Copied" on success, a plain sentence when the
// browser refuses; a long value wraps inside the row instead of scrolling the page.

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function clipboard(writeText: (t: string) => Promise<void> | undefined) {
  vi.stubGlobal("navigator", { clipboard: { writeText } });
}

describe("CopyValue", () => {
  it("shows the label and value and copies the value", async () => {
    const writeText = vi.fn(async () => {});
    clipboard(writeText);
    render(<CopyValue label="External id" value="org_7f3a" hint="Not a secret." />);
    expect(screen.getByText("External id")).toBeInTheDocument();
    expect(screen.getByTestId("copy-value-text")).toHaveTextContent("org_7f3a");
    expect(screen.getByText("Not a secret.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Copy External id" }));
    expect(writeText).toHaveBeenCalledWith("org_7f3a");
    await waitFor(() => expect(screen.getByRole("button", { name: "Copy External id" })).toHaveTextContent("Copied"));
  });

  it("says so when the browser refuses to copy", async () => {
    clipboard(async () => {
      throw new Error("denied");
    });
    render(<CopyValue label="Callback URL" value="https://aurora.example/v1/oauth/callback" />);
    fireEvent.click(screen.getByRole("button", { name: "Copy Callback URL" }));
    expect(await screen.findByText("Your browser blocked copying; select the text instead.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy Callback URL" })).toHaveTextContent("Copy");
  });

  it("says so when there is no clipboard at all", () => {
    clipboard(() => undefined);
    render(<CopyValue label="Callback URL" value="https://aurora.example/cb" />);
    fireEvent.click(screen.getByRole("button", { name: "Copy Callback URL" }));
    expect(screen.getByText("Your browser blocked copying; select the text instead.")).toBeInTheDocument();
  });

  it("wraps a long value and uses tokens only (no fixed width, no horizontal scroll)", () => {
    render(<CopyValue label="Callback URL" value={"https://aurora.example/" + "a".repeat(200)} />);
    const text = screen.getByTestId("copy-value-text");
    expect(text.className).toContain("break-all");
    expect(text.className).toContain("min-w-0");
    expect(screen.getByTestId("copy-value").innerHTML).not.toMatch(/overflow-x|w-\[|#[0-9a-f]{3,6}/i);
  });
});
