import { fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { RuleEditorDialog } from "@/components/policy/rule-editor";
import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));

afterEach(() => vi.unstubAllGlobals());

// (FIX-184, D-400) An organization owner or admin allows one named tool of an app that asks first by
// default: the rule editor's App tools target writes the existing `app` target with `tools`.
describe("Rule editor: App tools target", () => {
  it("names one tool of an app and saves the existing app target shape", async () => {
    mockFetch({
      "GET /v1/apps": () => json(200, { items: [{ app_id: "gmail", name: "Gmail" }] }),
      "GET /v1/apps/gmail": () =>
        json(200, {
          app_id: "gmail",
          tools: [
            { id: "list_messages", name: "List and search messages", group: "read" },
            { id: "send_message", name: "Send mail", group: "write", approval: "ask" },
          ],
        }),
      "GET /v1/policy/approval-limits": () => json(200, { parking_enabled: false, max_secs: 3600, threshold_secs: 0 }),
    });
    const onSave = vi.fn();
    wrap(
      <RuleEditorDialog
        open
        initial={{ name: "Allow Gmail send", enabled: true, action: "allow", identities: [], targets: [], conditions: [] } as never}
        data={{ agents: [], users: [], secrets: [] }}
        onClose={() => {}}
        onSave={onSave}
      />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Add target" }));
    fireEvent.change(await screen.findByLabelText("Target type"), { target: { value: "app" } });
    await screen.findByRole("option", { name: "Gmail" });
    fireEvent.change(await screen.findByLabelText("App"),{ target: { value: "gmail" } });
    expect(await screen.findByText("asks first by default")).toBeInTheDocument();
    fireEvent.click(await screen.findByLabelText(/Send mail/));
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(onSave).toHaveBeenCalled());
    const saved = onSave.mock.calls[0]![0] as { targets: unknown[] };
    expect(saved.targets).toEqual([{ type: "app", app_id: "gmail", tools: ["send_message"] }]);
  });
});
