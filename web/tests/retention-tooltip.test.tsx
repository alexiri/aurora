import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ChatPane } from "@/components/agents/chat-tab";
import { wrap } from "./support/render";
import { json } from "./support/api";

// (retention-tooltip) From 1280 px the org's retention window is the visibility line's tooltip in the thread header:
// hover, keyboard focus and a tap open it, Escape and blur close it, and while it shows the note is the line's
// accessible description. Below xl (and wherever there is no window) the header is what it was before. The
// breakpoint decision is `useMediaQuery`-only, so the tests stub `matchMedia` (jsdom has none); the note's text
// also lives in the inline segment (the meta line's, hidden at xl by CSS), so the tooltip is named by its test id.

const st = vi.hoisted(() => ({
  events: [] as unknown[],
}));
vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({
    state: { events: st.events, deltas: {}, activity: {}, lastSeq: 0 },
    loading: false,
    error: null,
    live: true,
  }),
}));

vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: vi.fn() }), useSearchParams: () => new URLSearchParams("") }));

const LINE = "All 2 people in this workspace can use this agent and read this conversation.";
const NOTE = "Deleted after 90 days without activity";

/** A desktop viewport: `matchMedia` answers the 1280 px query with `true`. */
function desktop() {
  vi.stubGlobal("matchMedia", (q: string) => ({ matches: q === "(min-width: 1280px)", media: q, addEventListener: () => {}, removeEventListener: () => {} }));
}

const pane = (props: Record<string, unknown> = {}) =>
  wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} agent={{ id: "a1", name: "Daneel", handle: "daneel", brief: "Triage the team's pull requests." }} audience={{ people: 2, owners: [{ id: "u9", display_name: "Ada Lovelace" }], owners_total: 1, emergency_active_more: 0 }} myRole="owner" readerId="u9" {...props} />);

describe("the retention tooltip on the visibility line (retention-tooltip)", () => {
  it("focus opens the tooltip, the note is the line's description, Escape and blur close it", () => {
    desktop();
    vi.stubGlobal("fetch", vi.fn(async () => json(404, { error: { code: "not_found", message: "nf" } })));
    pane({ retentionDays: 90 });
    // The line is the trigger; the tooltip is closed until it is hovered, focused or tapped.
    const line = screen.getByRole("button", { name: LINE });
    expect(screen.queryByTestId("retention-tip")).toBeNull();

    fireEvent.focus(line);
    const note = screen.getByTestId("retention-tip");
    expect(note).toHaveTextContent(NOTE);
    expect(line.getAttribute("aria-describedby")).toBe(note.id);

    fireEvent.keyDown(line, { key: "Escape" });
    expect(screen.queryByTestId("retention-tip")).toBeNull();

    fireEvent.focus(line);
    expect(screen.getByTestId("retention-tip")).toHaveTextContent(NOTE);
    fireEvent.blur(line);
    expect(screen.queryByTestId("retention-tip")).toBeNull();
  });

  it("hover and a tap open it too", () => {
    desktop();
    vi.stubGlobal("fetch", vi.fn(async () => json(404, { error: { code: "not_found", message: "nf" } })));
    pane({ retentionDays: 90 });
    const line = screen.getByRole("button", { name: LINE });

    fireEvent.mouseEnter(line);
    expect(screen.getByTestId("retention-tip")).toHaveTextContent(NOTE);
    fireEvent.keyDown(line, { key: "Escape" });
    expect(screen.queryByTestId("retention-tip")).toBeNull();

    // A touch tap focuses and clicks; blur (or a tap outside) closes.
    fireEvent.click(line);
    expect(screen.getByTestId("retention-tip")).toHaveTextContent(NOTE);
    fireEvent.blur(line);
    expect(screen.queryByTestId("retention-tip")).toBeNull();
  });

  it("without a retention window the line stays plain text: no trigger, no note", () => {
    desktop();
    vi.stubGlobal("fetch", vi.fn(async () => json(404, { error: { code: "not_found", message: "nf" } })));
    pane({ retentionDays: null });
    expect(screen.queryByRole("button", { name: LINE })).toBeNull();
    // The line itself is unchanged: the header's meta and the empty state under the greeting.
    expect(screen.getAllByText(LINE)).toHaveLength(2);
    expect(screen.queryByText(/Deleted after/)).toBeNull();
  });
});
