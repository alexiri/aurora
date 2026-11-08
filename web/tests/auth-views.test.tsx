import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let search = "";
const replace = vi.fn();
vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(search),
  useRouter: () => ({ replace }),
  usePathname: () => "/x",
}));

import { DeleteView } from "@/components/account/delete-view";
import { JoinView } from "@/components/auth/join-view";
import { LoginView } from "@/components/auth/login-view";
import { safeNext } from "@/lib/auth/use-unauthorized-redirect";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

const instance = {
  version: "1",
  signup_mode: "open",
  email_delivery: "smtp",
  setup_errors: [],
  oidc_providers: [{ id: "g", display_name: "Google" }],
};

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

function mockFetch(routes: Record<string, (init?: RequestInit) => Response>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const key = `${req?.method ?? init?.method ?? "GET"} ${url.pathname}`;
      const h = routes[key];
      if (!h) return json(404, { error: { code: "not_found", message: "nf" } });
      return h(init);
    }),
  );
}

beforeEach(() => {
  search = "";
  replace.mockReset();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("safeNext", () => {
  it("rejects non-local paths", () => {
    expect(safeNext("//evil.com")).toBe("/");
    expect(safeNext("https://evil.com")).toBe("/");
    expect(safeNext("/o/acme")).toBe("/o/acme");
    expect(safeNext(null)).toBe("/");
  });
});

describe("LoginView", () => {
  it("renders OIDC buttons only and maps callback error codes", async () => {
    search = "error=oidc_state_invalid&next=/o/acme";
    mockFetch({ "GET /v1/instance": () => json(200, instance) });
    wrap(<LoginView />);
    expect(screen.getByRole("alert")).toHaveTextContent(/single sign-on attempt expired/i);
    const link = await screen.findByRole("link", { name: /Continue with Google/ });
    expect(link).toHaveAttribute("href", "/v1/auth/oidc/g/start?return_to=%2Fo%2Facme");
    // RFE-004: no password form, no sign-up or reset links.
    expect(screen.queryByLabelText("Password")).toBeNull();
    expect(screen.queryByLabelText("Email")).toBeNull();
    expect(screen.queryByText(/forgot/i)).toBeNull();
    expect(screen.queryByText(/create an account/i)).toBeNull();
  });

  it.each([
    ["signup_closed", /Sign-up is closed/],
    ["email_unverified", /has not verified this email/],
    ["user_disabled", /account has been disabled/],
  ])("shows the %s error state", async (code, text) => {
    search = `error=${code}`;
    mockFetch({ "GET /v1/instance": () => json(200, instance) });
    wrap(<LoginView />);
    expect(screen.getByRole("alert")).toHaveTextContent(text);
    await screen.findByRole("link", { name: /Continue with Google/ });
  });

  it("labels the development provider with a banner", async () => {
    mockFetch({
      "GET /v1/instance": () =>
        json(200, {
          ...instance,
          oidc_providers: [{ id: "dev", display_name: "Development sign-in" }],
        }),
    });
    wrap(<LoginView />);
    const link = await screen.findByRole("link", { name: "Development sign-in" });
    expect(link).toHaveAttribute("href", "/v1/auth/oidc/dev/start?return_to=%2F");
    expect(screen.getByRole("status")).toHaveTextContent(/development sign-in is enabled/i);
  });

  it("says so when no provider is configured", async () => {
    mockFetch({ "GET /v1/instance": () => json(200, { ...instance, oidc_providers: [] }) });
    wrap(<LoginView />);
    expect(await screen.findByText(/No sign-in provider is configured/)).toBeInTheDocument();
  });
});

describe("JoinView", () => {
  const preview = {
    org: { id: "1", slug: "acme", name: "Acme" },
    email: "me@x.io",
    role: "member",
    inviter_name: "Ann",
    status: "pending",
    expires_at: "2030-01-01T00:00:00Z",
  };
  const unauthorized = () => json(401, { error: { code: "unauthorized", message: "x" } });

  it("offers sign-in only (no create account) carrying the token when signed out", async () => {
    search = "token=tok1";
    mockFetch({
      "GET /v1/invitations/preview": () => json(200, preview),
      "GET /v1/me": unauthorized,
    });
    wrap(<JoinView />);
    expect(await screen.findByRole("link", { name: "Sign in to accept" })).toHaveAttribute(
      "href",
      "/auth/login?next=%2Fjoin%3Ftoken%3Dtok1",
    );
    expect(screen.queryByRole("link", { name: "Create account" })).toBeNull();
  });

  it("shows expired state", async () => {
    search = "token=tok1";
    mockFetch({
      "GET /v1/invitations/preview": () => json(200, { ...preview, status: "expired" }),
      "GET /v1/me": unauthorized,
    });
    wrap(<JoinView />);
    expect(await screen.findByText(/has expired/)).toBeInTheDocument();
  });

  it("signed in: an unverified e-mail is explained, with no resend", async () => {
    search = "token=tok1";
    mockFetch({
      "GET /v1/invitations/preview": () => json(200, preview),
      "GET /v1/me": () =>
        json(200, { user: { email: "me@x.io", display_name: "Me" }, memberships: [] }),
      "GET /v1/instance": () => json(200, instance),
      "POST /v1/invitations/accept": () =>
        json(403, { error: { code: "email_unverified", message: "x" } }),
    });
    wrap(<JoinView />);
    fireEvent.click(await screen.findByRole("button", { name: "Accept invitation" }));
    expect(await screen.findByText(/has not verified this email/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Resend/ })).toBeNull();
  });

  it("signed in with a used invitation: the first sign-in already accepted it", async () => {
    search = "token=tok1";
    mockFetch({
      "GET /v1/invitations/preview": () =>
        json(404, { error: { code: "not_found", message: "nf" } }),
      "GET /v1/me": () =>
        json(200, { user: { email: "me@x.io", display_name: "Me" }, memberships: [] }),
    });
    wrap(<JoinView />);
    expect(await screen.findByText(/no longer pending/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open Aurora" })).toHaveAttribute("href", "/");
  });

  it("unknown token shows invalid link", async () => {
    search = "token=nope";
    mockFetch({ "GET /v1/me": unauthorized });
    wrap(<JoinView />);
    expect(await screen.findByText("This invitation link is not valid.")).toBeInTheDocument();
  });
});

describe("DeleteView", () => {
  it("412 reauth_required offers re-authentication through the provider", async () => {
    mockFetch({
      "GET /v1/instance": () => json(200, instance),
      "DELETE /v1/me": () =>
        json(412, { error: { code: "reauth_required", message: "x" } }),
    });
    wrap(<DeleteView />);
    expect(screen.queryByLabelText("Password")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Delete my account" }));
    const link = await screen.findByRole("link", { name: /Sign in again with Google/ });
    expect(link).toHaveAttribute(
      "href",
      "/v1/auth/oidc/g/start?return_to=%2Faccount%2Fdelete&reauth=1",
    );
  });
});
