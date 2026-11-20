import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { NotFoundState } from "@/components/not-found-state";
import { AppIcon } from "@/components/ui/app-icon";
import { Badge } from "@/components/ui/badge";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { SkeletonRows } from "@/components/ui/skeleton";
import { StatusLine } from "@/components/ui/status-line";
import { agentPathParts, safeFrom, sshKeysHref } from "@/lib/back-link";
import { lastScope, writeRecent } from "@/lib/nav/recent-scopes";

vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: vi.fn(), push: vi.fn() }), usePathname: () => "/", useParams: () => ({}) }));

afterEach(cleanup);

describe("AppIcon plate (admin-11)", () => {
  it("puts a logo without a dark variant on a light plate in the dark theme, and leaves a logo with one alone", () => {
    const { container } = render(<AppIcon appId="sentry" name="Sentry" />);
    const img = container.querySelector("img")!;
    expect(img.className).toContain("dark:bg-white/92");
    expect(img.className).toContain("dark:p-0.5");
    expect(img.className).toContain("dark:rounded-control");
    cleanup();
    const dark = render(<AppIcon appId="github" name="GitHub" hasDarkIcon />);
    for (const i of dark.container.querySelectorAll("img")) expect(i.className).not.toContain("white/92");
  });

  it("does not put a plate on the monogram tile", () => {
    const { container } = render(<AppIcon appId="x" name="Zed" hasIcon={false} />);
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByTestId("app-icon-fallback").className).not.toContain("white/92");
  });
});

describe("Dialog anchor (setup-7)", () => {
  it("is top-anchored at a fixed offset, never vertically centred", () => {
    render(
      <Dialog open>
        <DialogContent title="Add a key">
          <p>body</p>
        </DialogContent>
      </Dialog>,
    );
    const cls = screen.getByRole("dialog").className;
    expect(cls).toContain("sm:top-[12vh]");
    expect(cls).toContain("top-4");
    expect(cls).not.toContain("top-1/2");
    expect(cls).not.toContain("-translate-y-1/2");
    expect(cls).toContain("max-h-[calc(100dvh-2rem)]");
  });
});

describe("SkeletonRows (workspace-12)", () => {
  it("shows placeholder rows, announced once, with the pulse off for reduced motion", () => {
    const { container } = render(<SkeletonRows rows={3} />);
    expect(screen.getAllByTestId("skeleton-row")).toHaveLength(3);
    expect(screen.getByRole("status")).toHaveTextContent("Loading…");
    expect(container.innerHTML).toContain("motion-reduce:animate-none");
  });
});

describe("colour meaning (red = failure, amber = viewer can act, green = good news, neutral = information)", () => {
  it("maps Notice tones to the right palette", () => {
    const toneClass = (tone: "neutral" | "accent" | "pending" | "danger") => {
      const { container } = render(<Notice tone={tone}>x</Notice>);
      const cls = container.firstElementChild!.className;
      cleanup();
      return cls;
    };
    expect(toneClass("danger")).toContain("danger");
    expect(toneClass("pending")).toContain("pending");
    expect(toneClass("neutral")).not.toMatch(/danger|pending|accent/);
    expect(toneClass("accent")).toContain("accent");
  });

  it("maps Badge tones: Critical is danger, never accent (green)", () => {
    const { container } = render(<Badge tone="danger">Critical</Badge>);
    expect(container.firstElementChild!.className).toContain("danger");
    expect(container.firstElementChild!.className).not.toContain("accent");
  });

  it("maps StatusLine tones: fail is red, attention amber, ok and idle never red", () => {
    const text = (tone: "ok" | "fail" | "idle" | "attention") => {
      const { container } = render(<StatusLine tone={tone}>x</StatusLine>);
      const cls = container.firstElementChild!.className;
      cleanup();
      return cls;
    };
    expect(text("fail")).toContain("danger");
    expect(text("attention")).toContain("pending");
    expect(text("ok")).not.toMatch(/danger|pending/);
    expect(text("idle")).not.toMatch(/danger|pending/);
  });
});

describe("safeFrom (back link)", () => {
  it("accepts a same-origin /o/ path and nothing else", () => {
    expect(safeFrom("/o/acme/w/lab/agents/daneel")).toBe("/o/acme/w/lab/agents/daneel");
    expect(safeFrom("/o/acme/w/lab/agents/daneel?tab=ssh#x")).toBe("/o/acme/w/lab/agents/daneel?tab=ssh#x");
    for (const bad of ["//evil.test", "//evil.test/o/x", "https://evil.test/o/x", "javascript:alert(1)", "/\\evil.test", "/account", "", null, undefined, "/o/x\n/y", "/o/../..//evil.test/", "/o/https://evil.test"]) {
      expect(safeFrom(bad as string | null | undefined), String(bad)).toBeNull();
    }
  });
  it("builds the keys link and reads the agent from a path", () => {
    expect(sshKeysHref("/o/acme/w/lab/agents/daneel")).toBe("/account/ssh?from=%2Fo%2Facme%2Fw%2Flab%2Fagents%2Fdaneel");
    expect(agentPathParts("/o/acme/w/lab/agents/daneel/ssh")).toEqual({ org: "acme", ws: "lab", handle: "daneel" });
    expect(agentPathParts("/o/acme/settings")).toBeNull();
  });
});

describe("last scope (org-3)", () => {
  beforeEach(() => window.localStorage.clear());
  const memberships = [
    { org: { slug: "acme", name: "Acme" }, status: "active", workspaces: [{ slug: "lab", name: "Lab" }, { slug: "ops", name: "Ops" }] },
    { org: { slug: "umbrella", name: "Umbrella" }, status: "active", workspaces: [{ slug: "main", name: "Main" }] },
  ];
  it("is the newest recent workspace that still exists", () => {
    writeRecent("u1", "umbrella/main");
    writeRecent("u1", "gone/old");
    writeRecent("u1", "acme/ops");
    expect(lastScope("u1", memberships)).toEqual({ orgSlug: "acme", orgName: "Acme", wsSlug: "ops", wsName: "Ops" });
    writeRecent("u1", "acme/removed");
    expect(lastScope("u1", memberships)?.wsSlug).toBe("ops");
  });
  it("falls back to the first organization, and to nothing without one", () => {
    expect(lastScope("u2", memberships)).toEqual({ orgSlug: "acme", orgName: "Acme" });
    expect(lastScope("u2", [])).toBeUndefined();
    expect(lastScope("u2", undefined)).toBeUndefined();
  });
});

describe("NotFoundState (org-3)", () => {
  function setup(ui: React.ReactElement, me: unknown, instance: unknown = { org_creation: "instance_admins" }) {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const url = new URL(input instanceof Request ? input.url : String(input), "http://localhost");
        const body = url.pathname === "/v1/me" ? me : url.pathname === "/v1/instance" ? instance : {};
        return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
      }),
    );
    return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{ui}</QueryClientProvider>);
  }
  const me = (admin: boolean) => ({
    user: { id: "u1", email: "a@b.c", display_name: "A", is_instance_admin: admin },
    memberships: [{ org: { slug: "acme", name: "Acme" }, status: "active", role: "member", workspaces: [{ slug: "lab", name: "Lab" }] }],
  });

  it("says it couldn't find the organization, names the slug, and offers the viewer's own organization", async () => {
    window.localStorage.clear();
    setup(<NotFoundState kind="organization" slug="new" />, me(false));
    expect(screen.getByRole("heading", { name: "We couldn't find that organization" })).toBeInTheDocument();
    expect(screen.getByText(/There is no organization called “new”, or you don't have access to it\./)).toBeInTheDocument();
    const go = await screen.findByRole("link", { name: "Go to Acme" });
    expect(go.getAttribute("href")).toContain("/o/acme");
    expect(screen.queryByRole("button", { name: "Create an organization" })).toBeNull();
  });

  it("offers Create an organization where creating one is allowed", async () => {
    setup(<NotFoundState kind="organization" slug="new" />, me(true));
    expect(await screen.findByRole("button", { name: "Create an organization" })).toBeInTheDocument();
  });

  it("an unknown page says so with Go home", () => {
    setup(<NotFoundState kind="page" />, me(false));
    expect(screen.getByRole("heading", { name: "That page doesn't exist" })).toBeInTheDocument();
    expect(screen.getByText("Check the address, or go back to where you were.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go home" })).toBeInTheDocument();
  });
});
