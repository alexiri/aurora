import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AccountsList } from "@/components/accounts/accounts-list";
import { UndoToastHost } from "@/components/ui/undo-toast";
import { wrap } from "./support/render";
import { json } from "./support/api";

// (FIX-249, C-1168) The agent's "Add an AI key" dialog: when a key that already exists could be
// given, it opens on the first choice — **Use a key you already have** (the list; picking one and
// **Give <Agent> this key** grants it and closes with the toast) or **Add a new key** (today's
// flow). With no existing key the dialog opens straight on today's flow.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/apps",
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

type Handler = (req: Request) => Response | Promise<Response>;
let routes: Record<string, Handler>;

beforeEach(() => {
  routes = {};
  window.history.replaceState(null, "", "/o/acme/w/ops/apps");
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const key = `${req.method} ${new URL(req.url, "http://localhost").pathname}`;
      return routes[key] ? routes[key](req) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});

const aiKeys = (items: unknown[]) => {
  routes["GET /v1/agents/a1/ai-keys"] = () => json(200, { items });
};
const key = (id: string, name: string, scope: string, over: Record<string, unknown> = {}) => ({
  secret_id: id, name, scope, starter: false, usable: true, reason: null, ...over,
});
const open = () => {
  routes["GET /v1/workspaces/w1/accounts"] = () => json(200, { items: [], next_cursor: null });
  routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [], next_cursor: null });
  routes["GET /v1/apps"] = () => json(200, { items: [], next_cursor: null });
  routes["GET /v1/agents/a1/grants"] = () => json(200, { secrets: [], connections: [], converted_by_app: [] });
  wrap(
    <>
      <AccountsList scope={{ kind: "workspace", id: "w1" }} orgId="o1" canOwn agent={{ id: "a1", name: "Daneel" }} />
      <UndoToastHost />
    </>,
  );
};

const openDialog = async () => {
  open();
  fireEvent.click(await screen.findByRole("button", { name: "Add an AI key" }));
  return screen.findByRole("dialog", { hidden: false });
};

const providers = () =>
  json(200, {
    providers: [{ id: "litellm", display_name: "LiteLLM", dialects: [], auth_modes: ["api_key"] }],
    endpoints: [{ id: "aigw", provider: "litellm", display_name: "AI gateway", base_url: "https://x", dialects: [], permitted: true, key_scopes: ["workspace"] }],
  });

describe("the Add an AI key dialog with existing keys (FIX-249)", () => {
  it("opens on the first choice when an existing key could be given; the unusable one is disabled with its reason", async () => {
    aiKeys([
      { ...key("k2", "Sandbox key", "workspace"), starter: true },
      key("k3", "Old key", "organization", { usable: false, reason: "It does not work with this agent's model tool." }),
    ]);
    await openDialog();
    expect(await screen.findByText("Give Daneel an AI key")).toBeInTheDocument();
    expect(screen.getByText("Use a key you already have")).toBeInTheDocument();
    expect(screen.getByText("Add a new key")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText(/Use a key you already have/));
    expect(await screen.findByText("Which key should Daneel use?")).toBeInTheDocument();
    const starter = screen.getByLabelText(/Sandbox key/).closest("label")!;
    expect(starter.textContent).toContain("Workspace key");
    expect(starter.textContent).toContain("Starter");
    const old = screen.getByLabelText(/Old key/);
    expect(old.closest("label")!.textContent).toContain("It does not work with this agent's model tool.");
    expect(old).toBeDisabled();
  });

  it("choosing an existing key grants it and closes with the toast", async () => {
    aiKeys([key("k2", "Sandbox key", "workspace")]);
    let granted: unknown;
    routes["POST /v1/secrets/k2/grants"] = (req) => {
      granted = req.json();
      return json(200, { items: [{ agent_id: "a1", status: "created" }] });
    };
    await openDialog();
    fireEvent.click(await screen.findByLabelText(/Use a key you already have/));
    fireEvent.click(await screen.findByLabelText(/Sandbox key/));
    fireEvent.click(await screen.findByRole("button", { name: "Give Daneel this key" }));
    await waitFor(() => expect(granted).toBeTruthy());
    expect(await granted).toEqual({ agent_ids: ["a1"] });
    expect(await screen.findByText("Daneel can answer now")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("Add a new key is today's flow: the service choice, then the key fields", async () => {
    aiKeys([key("k2", "Sandbox key", "workspace")]);
    routes["GET /v1/llm/providers"] = providers;
    await openDialog();
    fireEvent.click(await screen.findByLabelText(/Add a new key/));
    expect(await screen.findByText("Which service is this key for?")).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: /^Next$/ })).toBeDisabled();
  });

  it("no existing key: today's flow only, straight on the service choice", async () => {
    aiKeys([]);
    routes["GET /v1/llm/providers"] = providers;
    await openDialog();
    expect(await screen.findByText("Which service is this key for?")).toBeInTheDocument();
    expect(screen.queryByText("Use a key you already have")).toBeNull();
  });
});
