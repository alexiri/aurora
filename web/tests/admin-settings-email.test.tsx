import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/settings/email",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { SettingsSectionView } from "@/components/admin/settings/settings-section-view";

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- request bodies are inspected loosely
type Json = Record<string, any>;
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = typeof input === "string" ? new Request(new URL(input, "http://localhost")) : (input as Request);
      const url = new URL(req.url, "http://localhost");
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const schema = (kind: string) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null });
const base = { locked: false, helm_key: "config.email.x", list: null, id_field: null, entries: null, default: null };

function email(o: { url?: string | null; passwordEnv?: boolean } = {}) {
  const url = o.url === undefined ? "smtp://mail.example.test:25" : o.url;
  return {
    section: "email",
    title: "Email",
    description: "The mail server.",
    generation: 2,
    updated_at: null,
    updated_by_label: null,
    extra: null,
    settings: [
      { ...base, key: "email.smtp_url", value: url, source: url ? "dashboard" : "default", schema: schema("string"), secret: null },
      { ...base, key: "email.from", value: "aurora@example.test", source: "dashboard", schema: schema("string"), secret: null },
      { ...base, key: "email.smtp_username", value: null, source: "default", schema: schema("string"), secret: null },
      { ...base, key: "email.smtp_password", value: null, source: "default", schema: schema("string"), secret: { set: !o.passwordEnv, from_env: o.passwordEnv ? "AURORA_SMTP_PASSWORD" : null } },
    ],
  };
}

const bodyOf = async (req: Request) => (await req.clone().json()) as Json;

describe("Admin › Settings › Email (SET-4)", () => {
  it("sends a test e-mail with the settings in force and shows the result", async () => {
    let body: Json | null = null;
    mockFetch({
      "GET /v1/admin/settings/email": () => json(200, email()),
      "POST /v1/admin/settings/email/test": async (req) => {
        body = await bodyOf(req);
        return json(200, { sent: true });
      },
    });
    render(
      <QueryClientProvider client={new QueryClient()}>
        <SettingsSectionView slug="email" />
      </QueryClientProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Send test e-mail" }));
    const r = await screen.findByRole("status", { name: "Test e-mail result" });
    expect(r.textContent).toContain("was sent to your address");
    expect(body).toEqual({});
    expect(screen.queryByText(/log-only mode/)).toBeNull();
  });

  it("sends the unsaved values, with a typed password as a one-off secret, and explains a failure", async () => {
    let body: Json | null = null;
    mockFetch({
      "GET /v1/admin/settings/email": () => json(200, email()),
      "POST /v1/admin/settings/email/test": async (req) => {
        body = await bodyOf(req);
        return json(200, { sent: false, error_code: "auth" });
      },
    });
    render(
      <QueryClientProvider client={new QueryClient()}>
        <SettingsSectionView slug="email" />
      </QueryClientProvider>,
    );
    fireEvent.change(await screen.findByLabelText("Smtp url"), { target: { value: "smtps://mail2.example.test:465" } });
    fireEvent.click(screen.getByRole("button", { name: "Replace…" }));
    fireEvent.change(screen.getByLabelText("Smtp password: new value"), { target: { value: "pw-canary" } });
    fireEvent.click(screen.getByRole("button", { name: "Send test e-mail" }));
    const r = await screen.findByRole("status", { name: "Test e-mail result" });
    expect(r.textContent).toContain("refused the user name or password");
    await waitFor(() => expect(body).not.toBeNull());
    expect(body!.values["email.smtp_url"]).toBe("smtps://mail2.example.test:465");
    expect(body!.values["email.smtp_password"]).toEqual({ $secret: "pw-canary" });
  });

  it("shows a notice in log-only mode and the password as locked when the environment sets it", async () => {
    mockFetch({ "GET /v1/admin/settings/email": () => json(200, email({ url: null, passwordEnv: true })) });
    render(
      <QueryClientProvider client={new QueryClient()}>
        <SettingsSectionView slug="email" />
      </QueryClientProvider>,
    );
    expect(await screen.findByText(/log-only mode/)).toBeTruthy();
    expect(screen.getByText("Set from the environment")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Set…" })).toBeNull();
  });
});
