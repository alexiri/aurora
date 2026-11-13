import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PlacementView } from "@/components/admin/placement-view";
import { ERROR_MESSAGES } from "@/lib/api/errors";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const ORG = "00000000-0000-4000-8000-000000000001";
const KEY = "00000000-0000-4000-8000-000000000009";

const unsaved = {
  saved: false,
  signup_mode: "open",
  mode: "personal_org",
  default_org: null,
  default_org_role: "member",
  shared_workspaces: [],
  personal_workspace: { enabled: false, role: "owner", name_template: "{sub}|{email_local}" },
  starter_grants: [],
  personal_orgs: {
    count: 2,
    items: [
      { org_id: "o1", slug: "ada-s-org", name: "Ada's org", owner_email: "ada@example.org", created_at: "2026-11-01T00:00:00Z" },
      { org_id: "o2", slug: "bob-s-org", name: "Bob's org", owner_email: "bob@example.org", created_at: "2026-11-02T00:00:00Z" },
    ],
  },
  warnings: ["placement_unsaved", "per_user_orgs_active"],
  updated_at: null,
  updated_by: null,
};

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

/** Routes the placement API by path; records every PUT body. */
function stubApi(initial: unknown = unsaved) {
  const puts: unknown[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url);
      const p = url.pathname;
      if (p === "/v1/admin/placement" && input.method === "PUT") {
        const body = await input.json();
        puts.push(body);
        return json(200, { ...unsaved, saved: true, mode: body.mode, warnings: [], updated_at: "2026-11-10T10:00:00Z" });
      }
      if (p === "/v1/admin/placement") return json(200, initial);
      if (p === "/v1/admin/placement/preview") {
        const sample = url.searchParams.get("sample") ?? "";
        const tpl = url.searchParams.get("template") ?? "";
        const name = tpl.includes("{name}") ? sample : sample.toLowerCase();
        return json(200, { name, slug: name.toLowerCase().replace(/[^a-z0-9]+/g, "-"), used: 0, unresolved: [] });
      }
      if (p === "/v1/admin/placement/choices") {
        return json(200, {
          workspaces: [{ id: "w1", slug: "general", name: "General" }],
          secrets: [{ id: KEY, name: "org-llm-key", kind: "llm" }],
          connections: [],
        });
      }
      if (p === "/v1/admin/orgs") return json(200, { items: [{ id: ORG, slug: "users", name: "Users", created_at: "2026-11-01T00:00:00Z" }], next_cursor: null });
      return json(404, { error: { code: "not_found", message: "x" } });
    }),
  );
  return puts;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("Admin › Sign-up & placement (P4-ONB-1, D-140)", () => {
  it("warns while per-user organizations are active and lists the ones from before", async () => {
    stubApi();
    wrap(<PlacementView />);
    expect(await screen.findByText(/nothing is saved yet/i)).toBeInTheDocument();
    expect(screen.getByText("2 personal organizations from before.")).toBeInTheDocument();
    expect(screen.getByText("ada@example.org")).toBeInTheDocument();
  });

  it("proposes shared organization Users with personal workspaces and previews the name", async () => {
    stubApi();
    wrap(<PlacementView />);
    const org = (await screen.findByLabelText("Organization")) as HTMLSelectElement;
    await waitFor(() => expect(org.value).toBe(ORG));
    expect(screen.getByLabelText("Give every user a workspace of their own")).toBeChecked();
    // The hint lists every variable, `{sub}` first (FIX-045/D-162).
    expect(
      screen.getByText(
        "Variables: {sub} {preferred_username} {name} {email_local} {given_name} {family_name} {claim:path}. Separate fallbacks with |.",
      ),
    ).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Try it with a name"), { target: { value: "Ada Lovelace" } });
    fireEvent.change(screen.getByLabelText("Workspace name"), { target: { value: "{name}" } });
    await waitFor(() => expect(screen.getByTestId("placement-preview")).toHaveTextContent("workspace Ada Lovelace (ada-lovelace)"));
  });

  it("saves the form with workspaces and starter grants", async () => {
    const puts = stubApi();
    wrap(<PlacementView />);
    await waitFor(() => expect((screen.getByLabelText("Organization") as HTMLSelectElement).value).toBe(ORG));
    fireEvent.click(await screen.findByLabelText("General"));
    fireEvent.click(await screen.findByLabelText("org-llm-key"));
    fireEvent.click(screen.getByRole("button", { name: "Save placement" }));
    expect(await screen.findByText(/new sign-ins follow it/i)).toBeInTheDocument();
    expect(puts).toEqual([
      {
        mode: "shared_org",
        default_org_id: ORG,
        default_org_role: "member",
        shared_workspaces: [{ workspace_id: "w1", role: "member" }],
        personal_workspace: { enabled: true, role: "owner", name_template: "{sub}|{email_local}" },
        starter_grants: [{ kind: "secret", id: KEY }],
      },
    ]);
  });

  it("sends only the mode for own organization and no placement", async () => {
    const puts = stubApi();
    wrap(<PlacementView />);
    await screen.findByLabelText("Organization");
    fireEvent.click(screen.getByLabelText("Own organization"));
    expect(screen.queryByLabelText("Organization")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save placement" }));
    await screen.findByText(/new sign-ins follow it/i);
    expect(puts).toEqual([{ mode: "personal_org" }]);
  });

  it("says in words why a new user waits on a fresh instance", () => {
    expect(ERROR_MESSAGES.instance_setup_pending).toBe("This instance is being set up — try again shortly.");
  });
});
