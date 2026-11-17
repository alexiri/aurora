import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AvailabilityEditor } from "@/components/apps/availability-view";
import { DynamicForm } from "@/components/apps/dynamic-form";
import { Markdown, safeHref } from "@/components/ui/markdown";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import {
  buildClientPatchFields,
  buildConnectFields,
  parseConnectMessage,
  parseJsonFile,
  toggleAllow,
  validateFields,
  type FormField,
} from "@/lib/api/apps";

vi.mock("next/navigation", () => ({ useParams: () => ({ org: "o1", ws: "w1" }), useSearchParams: () => new URLSearchParams() }));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const f = (over: Partial<FormField>): FormField => ({
  name: "x", label: "X", type: "text", store: "credentials", required: false, ...over,
});

const FIELDS: FormField[] = [
  f({ name: "mode", label: "Mode", type: "select", required: true, options: [{ value: "a", label: "A", warning: null }, { value: "b", label: "B", warning: "B is risky" }] }),
  f({ name: "token", label: "Token", type: "secret", required: true, show_when: { field: "mode", equals: "a" } }),
  f({ name: "key", label: "Key file", type: "json_file", required: true, show_when: { field: "mode", equals: "b" } }),
];

describe("dynamic form", () => {
  it("honours show_when and required for visible fields only", () => {
    expect(Object.keys(validateFields(FIELDS, { mode: "a" }))).toEqual(["token"]);
    expect(Object.keys(validateFields(FIELDS, { mode: "b" }))).toEqual(["key"]);
    expect(validateFields(FIELDS, { mode: "a", token: "t" })).toEqual({});
  });
  it("never submits hidden field values", () => {
    expect(buildConnectFields(FIELDS, { mode: "a", token: "t", key: "{}" })).toEqual({ mode: "a", token: "t" });
  });
  it("validates json_file content", () => {
    expect(parseJsonFile('{"a":1}').ok).toBe(true);
    expect(parseJsonFile("[1]").ok).toBe(false);
    expect(parseJsonFile("nope").ok).toBe(false);
    expect(validateFields(FIELDS, { mode: "b", key: "nope" }).key).toMatch(/not valid JSON/);
  });
  it("renders conditional fields, select warnings and secret inputs", () => {
    const onChange = vi.fn();
    const { rerender } = render(<DynamicForm fields={FIELDS} values={{ mode: "a" }} errors={{}} onChange={onChange} />);
    expect(screen.getByLabelText("Token")).toHaveAttribute("type", "password");
    expect(screen.queryByLabelText("Key file")).toBeNull();
    rerender(<DynamicForm fields={FIELDS} values={{ mode: "b" }} errors={{}} onChange={onChange} />);
    expect(screen.queryByLabelText("Token")).toBeNull();
    expect(screen.getByLabelText("Key file")).toBeInTheDocument();
    expect(screen.getByText("B is risky")).toBeInTheDocument();
  });
});

describe("connect channel messages (C-842)", () => {
  it("accepts the callback page's two shapes and nothing else", () => {
    expect(parseConnectMessage({ type: "aurora:connected", app_id: "a", connection_id: "c1", missing_scopes: ["x"] })).toEqual({
      type: "connected", appId: "a", connectionId: "c1", missingScopes: ["x"],
    });
    expect(parseConnectMessage({ type: "aurora:connect-failed", app_id: "a", error: "oauth_exchange_failed" })).toEqual({
      type: "failed", appId: "a", error: "oauth_exchange_failed",
    });
    expect(parseConnectMessage({ type: "aurora:connect-failed", error: "oauth_state_invalid" })).toEqual({
      type: "failed", appId: null, error: "oauth_state_invalid",
    });
    expect(parseConnectMessage({ type: "aurora:connected", connection_id: "c1" })).toBeNull();
    expect(parseConnectMessage({ type: "aurora:connect-ack", connection_id: "c1" })).toBeNull();
    expect(parseConnectMessage({ type: "other" })).toBeNull();
    expect(parseConnectMessage(null)).toBeNull();
  });
});

describe("markdown", () => {
  it("does not render disallowed raw HTML or unsafe links, and keeps the full target in the tooltip", () => {
    const { container } = render(<Markdown source={'<script>x</script>\n\n[ok](https://e.com/a) [bad](javascript:alert(1))'} />);
    expect(container.querySelector("script")).toBeNull();
    // D-227: raw HTML outside the allowlist is dropped (a script's content with it, as on github.com).
    expect(container.textContent).not.toContain("<script>");
    const links = container.querySelectorAll("a");
    expect(links).toHaveLength(1);
    expect(links[0]).toHaveAttribute("href", "https://e.com/a");
    expect(links[0]).toHaveAttribute("title", "https://e.com/a");
    expect(container.textContent).toContain("bad");
    expect(safeHref("javascript:alert(1)")).toBeNull();
  });
});

describe("availability editor", () => {
  it("toggles allow entries", () => {
    expect(toggleAllow([], "a", null)).toEqual([{ app_id: "a" }]);
    expect(toggleAllow([{ app_id: "a" }], "a", null)).toEqual([]);
  });
  const availabilityFetch = (state: { posture: string; allow: unknown[] }, puts: unknown[]) =>
    vi.stubGlobal("fetch", vi.fn(async (input: Request) => {
      if (input.method === "PUT") {
        const b = (await input.json()) as { posture: string; allow: unknown[] };
        puts.push(b);
        return json(200, b);
      }
      return json(200, state);
    }));

  it("PUTs the whole list with the posture as it is (restricted: the toggles show)", async () => {
    const puts: unknown[] = [];
    availabilityFetch({ posture: "restricted", allow: [] }, puts);
    wrap(<AvailabilityEditor orgId="o1" orgSlug="acme" initial={{ posture: "restricted", allow: [] }} apps={[{ app_id: "gh", name: "GitHub" }]} canEdit />);
    // the posture is not set here any more (FIX-130): no radio, a link to the posture panel on Apps & accounts › Apps
    expect(screen.queryByLabelText(/Restricted/)).toBeNull();
    expect(screen.getByRole("link", { name: "Apps & accounts › Apps" })).toHaveAttribute("href", "/o/acme/apps?tab=apps");
    fireEvent.click(screen.getByLabelText("GitHub"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts).toEqual([{ posture: "restricted", allow: [{ app_id: "gh" }] }]));
    await screen.findByText("Saved.");
  });

  it("while the posture is open it says so and links to the posture, with no toggles", () => {
    availabilityFetch({ posture: "open", allow: [] }, []);
    wrap(<AvailabilityEditor orgId="o1" orgSlug="acme" initial={{ posture: "open", allow: [] }} apps={[{ app_id: "gh", name: "GitHub" }]} canEdit />);
    expect(screen.getByText(/Every app is available/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Apps & accounts › Apps" })).toHaveAttribute("href", "/o/acme/apps?tab=apps");
    expect(screen.queryByLabelText("GitHub")).toBeNull();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });
});

describe("oauth client secrets", () => {
  const client = { id: "c1", scope: "workspace", package_id: "p", client_key: "main", fields: { client_id: "abc" }, enabled: true, connection_count: 0, created_at: "", updated_at: "" };
  it("omits untouched secrets on patch", () => {
    expect(buildClientPatchFields({ client_secret: "", client_id: "n" })).toEqual({ client_id: "n" });
  });
  it("edit form is built from the definition; secrets are password inputs, never prefilled", async () => {
    const detail = {
      app_id: "a1", package_id: "p", name: "App", connect: [],
      oauth_clients: [{ client_key: "main", setup_note: "Create an app first.", redirect_uri: "https://x.test/v1/oauth/callback",
        fields: [{ name: "client_id", label: "Client ID", secret: false }, { name: "client_secret", label: "Client secret", secret: true }] }],
    };
    vi.stubGlobal("fetch", vi.fn(async (input: Request) => {
      const u = new URL(input.url);
      if (u.pathname === "/v1/workspaces/w1/oauth-clients") return json(200, { items: [{ ...client, fields: { client_id: "abc", client_secret: "" } }] });
      if (u.pathname === "/v1/apps") return json(200, { items: [{ app_id: "a1", package_id: "p" }] });
      if (u.pathname === "/v1/apps/a1") return json(200, detail);
      return json(200, { items: [] });
    }));
    wrap(<OAuthClientsPanel scope={{ kind: "workspace", id: "w1" }} canOwn />);
    await screen.findByText("main", { exact: false });
    await waitFor(() => fireEvent.click(screen.getByRole("button", { name: "Edit" })));
    const secret = await screen.findByLabelText("Client secret");
    expect(secret).toHaveAttribute("type", "password");
    expect(secret).toHaveValue("");
    expect(screen.getByLabelText("Client ID")).toHaveValue("abc");
    expect(screen.getByText("Create an app first.")).toBeInTheDocument();
    expect(screen.getByText("https://x.test/v1/oauth/callback")).toBeInTheDocument();
  });
});
