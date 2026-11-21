import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { GeneralTab } from "@/components/settings/general-tab";
import { queryKeys, useWorkspace } from "@/lib/api/queries";
import { seedMoreControls } from "./support/render";
import { json } from "./support/api";

// P5-REFINE-16 workspace-7: renaming says so, and the switcher's source (`me`) has the new name at once.

const workspace = {
  id: "w1", org_id: "o1", slug: "lab", name: "Your workspace", timezone: "UTC", memory_language: "english",
  byo_posture: null, effective_byo_posture: { value: "owners_only", level: "instance" },
  my_role: "owner", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
} as never;

let current = { ...(workspace as object) } as Record<string, unknown>;

describe("General rename", () => {
  it("shows the saved sentence and patches the cached me before the refetch", async () => {
    // `useMe` calls fetch with a plain path, so this stub reads both forms. The refetch of `me` never answers, so only the patch can
    // have put the new name in the cache.
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const req = input instanceof Request ? input : null;
        const url = new URL(req ? req.url : String(input), "http://localhost");
        const key = `${req?.method ?? "GET"} ${url.pathname}`;
        // The workspace as the server holds it: a save changes `name` and `updated_at`, and the page's query reads it back.
        if (key === "PATCH /v1/workspaces/w1") {
          current = { ...current, name: "Bob's Lab", updated_at: "2026-01-02T00:00:00Z" };
          return json(200, current);
        }
        if (key === "GET /v1/workspaces/w1") return json(200, current);
        if (key === "GET /v1/workspaces/w1/agents") return json(200, { items: [{ id: "a", name: "Dani", handle: "dani" }], next_cursor: null });
        if (key === "GET /v1/me") return new Promise<Response>(() => {});
        return json(404, { error: { code: "not_found", message: "nf" } });
      }),
    );
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    seedMoreControls(qc, true);
    qc.setQueryData(["me"], { user: { id: "u" }, memberships: [{ workspaces: [{ id: "w1", name: "Your workspace" }] }] });
    qc.setQueryDefaults(["me"], { staleTime: Infinity });
    // As the workspace page does: the form gets the workspace the page's query holds, and it is not remounted when that changes.
    function Page() {
      const w = useWorkspace("w1");
      return w.data ? <GeneralTab workspace={w.data} canOwn /> : null;
    }
    render(
      <QueryClientProvider client={qc}>
        <Page />
      </QueryClientProvider>,
    );
    // (workspace-7) One label for the workspace form, and the language in words, not "english".
    const language = (await screen.findByLabelText("Language agents remember in")) as HTMLSelectElement;
    expect(language.selectedOptions[0]!.textContent).toBe("English");
    expect(Array.from(language.options).map((o) => o.textContent)).not.toContain("english");
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Bob's Lab" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByTestId("saved-name")).toHaveTextContent("Saved. The workspace is now called Bob's Lab.");
    await waitFor(() =>
      expect((qc.getQueryData(["me"]) as { memberships: { workspaces: { name: string }[] }[] }).memberships[0]!.workspaces[0]!.name).toBe("Bob's Lab"),
    );
    // The workspace was read back with its new `updated_at`: the sentence is still there, and the form shows the stored name.
    await waitFor(() => expect(qc.getQueryData<{ updated_at: string }>(queryKeys.workspace("w1"))?.updated_at).toBe("2026-01-02T00:00:00Z"));
    expect(screen.getByTestId("saved-name")).toHaveTextContent("Saved. The workspace is now called Bob's Lab.");
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Bob's Lab");
  });
});
