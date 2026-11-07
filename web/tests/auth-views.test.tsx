import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let search = "";
const replace = vi.fn();
vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(search),
  useRouter: () => ({ replace }),
  usePathname: () => "/x",
}));

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
  it("renders OIDC buttons and maps callback error codes", async () => {
    search = "error=oidc_state_invalid&next=/o/acme";
    mockFetch({ "GET /v1/instance": () => json(200, instance) });
    wrap(<LoginView />);
    expect(screen.getByRole("alert")).toHaveTextContent(/single sign-on attempt expired/i);
    const link = await screen.findByRole("link", { name: /Continue with Google/ });
    expect(link).toHaveAttribute("href", "/v1/auth/oidc/g/start?return_to=%2Fo%2Facme");
  });

  it("shows field errors under inputs", async () => {
    mockFetch({
      "GET /v1/instance": () => json(200, instance),
      "POST /v1/auth/login": () =>
        json(400, {
          error: {
            code: "invalid_request",
            message: "bad",
            details: { fields: [{ path: "email", message: "not an email" }] },
          },
        }),
    });
    wrap(<LoginView />);
    fireEvent.change(screen.getByLabelText("Email"), { target: { value: "x" } });
    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "y" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("not an email")).toBeInTheDocument();
    expect(screen.getByLabelText("Email")).toHaveAttribute("aria-invalid", "true");
    expect(replace).not.toHaveBeenCalled();
  });

  it("maps invalid_credentials and redirects to next on success", async () => {
    search = "next=/o/acme";
    let ok = false;
    mockFetch({
      "GET /v1/instance": () => json(200, instance),
      "POST /v1/auth/login": () =>
        ok
          ? json(200, { user: {} })
          : json(401, { error: { code: "invalid_credentials", message: "x" } }),
    });
    wrap(<LoginView />);
    fireEvent.change(screen.getByLabelText("Email"), { target: { value: "a@b.c" } });
    fireEvent.change(screen.getByLabelText("Password"), { target: { value: "pw" } });
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("The email or password is incorrect.")).toBeInTheDocument();
    ok = true;
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    await waitFor(() => expect(replace).toHaveBeenCalledWith("/o/acme"));
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

  it("offers sign in / create account carrying the token when signed out", async () => {
    search = "token=tok1";
    mockFetch({
      "GET /v1/invitations/preview": () => json(200, preview),
      "GET /v1/me": unauthorized,
    });
    wrap(<JoinView />);
    expect(await screen.findByRole("link", { name: "Sign in" })).toHaveAttribute(
      "href",
      "/auth/login?next=%2Fjoin%3Ftoken%3Dtok1",
    );
    expect(screen.getByRole("link", { name: "Create account" })).toHaveAttribute(
      "href",
      "/auth/signup?invitation=tok1",
    );
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

  it("signed in: email_unverified offers resend", async () => {
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
    expect(
      await screen.findByRole("button", { name: "Resend verification email" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Verify your email address first.")).toBeInTheDocument();
  });

  it("unknown token shows invalid link", async () => {
    search = "token=nope";
    mockFetch({ "GET /v1/me": unauthorized });
    wrap(<JoinView />);
    expect(await screen.findByText("This invitation link is not valid.")).toBeInTheDocument();
  });
});
