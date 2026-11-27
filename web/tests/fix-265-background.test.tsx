import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { TranscriptView } from "@/components/chat/transcript-view";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { applyEvents, emptyTranscript } from "@/lib/chat/transcript";
import { wrap } from "./support/render";

const ev = (seq: number, payload: Record<string, unknown>): TranscriptEvent => ({
  seq,
  type: "notice",
  ts: "2026-01-01T00:00:00Z",
  payload,
});

const activity = (seq: number, command: string) =>
  ev(seq, { code: "background_activity", turn_id: `t${seq}`, items: [{ command, started_at: 1700000000 }], text: `Background activity, still running: \`${command}\`` });

const view = (events: TranscriptEvent[]) => wrap(<TranscriptView state={applyEvents(emptyTranscript, events)} wsId="w" base="/b" convId="c1" />);

// (FIX-265) The list shows what still runs: a newer entry replaces the older one.
describe("background activity", () => {
  it("shows only the newest entry, with its own Stop", () => {
    view([activity(1, "cmd-a"), activity(2, "cmd-b")]);
    const rows = screen.getAllByTestId("background-activity");
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain("cmd-b");
    expect(rows[0].textContent).not.toContain("cmd-a");
    expect(screen.getAllByRole("button", { name: "Stop" })).toHaveLength(1);
  });

  it("shows an entry again for work that starts after an earlier one ended", () => {
    view([activity(1, "cmd-a"), ev(2, { code: "background_stopped", turn_id: "t1", text: "Background activity stopped." }), activity(3, "cmd-b")]);
    const rows = screen.getAllByTestId("background-activity");
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain("cmd-b");
  });
});
