import { fireEvent, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ProfileView } from "@/components/account/profile-view";
import { useTimezoneAutofill } from "@/lib/timezone";
import { json } from "./support/api";
import { wrap } from "./support/render";

// P5-PERSON (D-402(d), C-1054): the personal time zone in Account › Profile, and the app shell's
// one-time fill from the browser when it is unset.

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));
vi.mock("next/link", () => ({
  default: ({ href, children, ...rest }: { href: string; children: ReactNode }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));

type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
    const url = new URL(req.url);
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

const me = (timezone: string | null) => ({
  approval_email: true,
  admin_attention_email: true,
  client_address: null,
  default_org_id: null,
  groups: [],
  idp_roles: null,
  memberships: [],
  timezone,
  user: { id: "u1", email: "a@b.c", display_name: "Ann", created_at: "2026-01-01T00:00:00Z" },
});

beforeEach(() => {
  window.sessionStorage.clear();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("Account › Profile time zone", () => {
  it("shows the stored zone and saves a change; Not set clears it", async () => {
    const bodies: unknown[] = [];
    mockFetch({
      "GET /v1/me": () => json(200, me("Europe/Zurich")),
      "GET /v1/instance": () => json(200, { features: {} }),
      "PATCH /v1/me": async (req) => {
        bodies.push(await req.json());
        return json(200, me("America/New_York"));
      },
    });
    wrap(<ProfileView />);
    const select = await screen.findByLabelText("Time zone");
    expect(select).toHaveValue("Europe/Zurich");
    fireEvent.change(select, { target: { value: "America/New_York" } });
    fireEvent.click(screen.getByRole("button", { name: "Save", exact: true }));
    await waitFor(() => expect(bodies).toHaveLength(1));
    expect(bodies[0]).toMatchObject({ timezone: "America/New_York" });

    fireEvent.change(await screen.findByLabelText("Time zone"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(bodies).toHaveLength(2));
    expect(bodies[1]).toMatchObject({ timezone: null });
  });
});

function Shell() {
  useTimezoneAutofill();
  return <p>shell</p>;
}

function browserZone(tz: string) {
  const real = Intl.DateTimeFormat.prototype.resolvedOptions;
  vi.spyOn(Intl.DateTimeFormat.prototype, "resolvedOptions").mockImplementation(function (this: Intl.DateTimeFormat) {
    return { ...real.call(this), timeZone: tz };
  });
}

describe("the app shell's time zone fill", () => {
  it("sends exactly one PATCH with the browser's zone when the stored one is null", async () => {
    browserZone("Europe/Zurich");
    const bodies: unknown[] = [];
    mockFetch({
      "GET /v1/me": () => json(200, me(null)),
      "PATCH /v1/me": async (req) => {
        bodies.push(await req.json());
        return json(200, me(null));
      },
    });
    const first = wrap(<Shell />);
    await waitFor(() => expect(bodies).toHaveLength(1));
    expect(bodies[0]).toEqual({ timezone: "Europe/Zurich" });
    // The same browser session, a new page load: no second PATCH even though the API still says null.
    first.unmount();
    wrap(<Shell />);
    await new Promise((r) => setTimeout(r, 50));
    expect(bodies).toHaveLength(1);
  });

  it("sends none when the zone is set", async () => {
    browserZone("Europe/Zurich");
    const fn = mockFetch({ "GET /v1/me": () => json(200, me("Asia/Tokyo")) });
    wrap(<Shell />);
    await waitFor(() => expect(fn).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 50));
    const methods = fn.mock.calls.map(([input]) => (input instanceof Request ? input.method : "GET"));
    expect(methods).not.toContain("PATCH");
  });

  it("sends none when the browser's zone is not a valid IANA name", async () => {
    browserZone("Etc/Unknown");
    const fn = mockFetch({ "GET /v1/me": () => json(200, me(null)) });
    wrap(<Shell />);
    await waitFor(() => expect(fn).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 50));
    const methods = fn.mock.calls.map(([input]) => (input instanceof Request ? input.method : "GET"));
    expect(methods).not.toContain("PATCH");
  });
});
