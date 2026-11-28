import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ThreadHeader } from "@/components/chat/thread-header";

// The thread's meta line wraps below 1280 px. Each dot belongs to the segment before it, so no row starts with one.

/** The text as read: every run of white space (the no-break space before a dot included) is one space. */
const read = (e: Element) => (e.textContent ?? "").replace(/\s+/g, " ").trim();
/** The meta line's wrapping pieces: the visibility note (with its icon) and one per segment. */
const pieces = () => [...screen.getByTestId("thread-meta").children].map(read);

describe("ThreadHeader's meta line", () => {
  it("never starts a wrapping piece with the dot: it ends the one before", () => {
    render(<ThreadHeader title="Direct chat" visibility="Only you can see this." segments={["Archived 21 Nov 2026", "read only"]} live={null} />);
    expect(pieces()).toEqual(["Only you can see this. ·", "Archived 21 Nov 2026 ·", "read only"]);
    expect(read(screen.getByTestId("thread-meta"))).toBe("Only you can see this. · Archived 21 Nov 2026 · read only");
  });

  it("without a visibility note, the segments alone keep the same rule", () => {
    render(<ThreadHeader title="Run" segments={["Schedule", "Schedule deleted"]} live={null} />);
    expect(pieces()).toEqual(["Schedule ·", "Schedule deleted"]);
  });

  it("the dot before a segment that hides at some width hides with it", () => {
    render(<ThreadHeader title="Chat" visibility="Only you can see this." segments={[{ text: "Deleted after 30 days without activity", className: "xl:hidden" }]} live={null} />);
    const dot = screen.getByText("·", { exact: false, selector: "[aria-hidden='true']" });
    expect(dot).toHaveClass("xl:hidden");
  });
});
