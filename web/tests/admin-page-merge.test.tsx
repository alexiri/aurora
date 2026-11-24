import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { json } from "./support/api";

vi.mock("next/navigation", () => ({
  usePathname: () => "/x",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(window.location.search),
  useParams: () => ({}),
}));

import { AdminAiModelsPage, AdminInternetAccessPage, AdminPlatformPage } from "@/components/admin/admin-topics";
import { TABS, adminRoutes } from "@/lib/routes";

// P5-REFINE-18 #1 (rec 9): What agents may do is one page with four tabs; Computers is a tab of Health & runners.

function mock() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const url = new URL((input as Request).url, "http://localhost");
      if (url.pathname === "/v1/me") return json(200, { user: { id: "u1", email: "a@x.io", is_instance_admin: true }, memberships: [] });
      // Every tab body shows its own error state: the test is about the page's tabs.
      return json(404, { error: { code: "not_found", message: "Not found." } });
    }),
  );
}

afterEach(() => {
  window.history.replaceState(null, "", "/");
});

function wrap(ui: React.ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("route helpers and tab ids", () => {
  it("the merged pages' tabs and addresses", () => {
    expect(TABS.adminAgentPolicy).toEqual(["internet-access", "ai-keys", "byo", "defaults", "templates"]);
    expect(TABS.adminPlatform).toEqual(["health", "runners", "computers", "secret-stores", "email", "limits"]);
    expect(adminRoutes.agentPolicy("byo")).toBe("/admin/internet-access?tab=byo");
    expect(adminRoutes.agentPolicy()).toBe("/admin/internet-access");
    expect(adminRoutes.platform("computers")).toBe("/admin/platform?tab=computers");
  });
});

describe("What agents may do", () => {
  it("has the four tabs in the brief's order, opening on Internet access", async () => {
    window.history.replaceState(null, "", "/admin/internet-access");
    mock();
    wrap(<AdminInternetAccessPage />);
    await screen.findByRole("heading", { name: "What agents may do" });
    const tabs = (await screen.findAllByRole("tab")).map((t) => t.textContent);
    expect(tabs).toEqual(["Internet access", "AI keys and rules", "Bring your own", "Defaults"]);
    expect(screen.getByRole("tab", { name: "Internet access" })).toHaveAttribute("aria-selected", "true");
  });

  it.each([
    ["ai-keys", "AI keys and rules"],
    ["byo", "Bring your own"],
    ["defaults", "Defaults"],
  ])("?tab=%s selects %s", async (tab, name) => {
    window.history.replaceState(null, "", `/admin/internet-access?tab=${tab}`);
    mock();
    wrap(<AdminInternetAccessPage />);
    expect(await screen.findByRole("tab", { name })).toHaveAttribute("aria-selected", "true");
  });
});

describe("Health & runners", () => {
  it("lists Health, Runners and Computers first, and Computers opens by its address", async () => {
    window.history.replaceState(null, "", "/admin/platform?tab=computers");
    mock();
    wrap(<AdminPlatformPage />);
    await screen.findByRole("heading", { name: "Health & runners" });
    const tabs = (await screen.findAllByRole("tab")).map((t) => t.textContent);
    expect(tabs.slice(0, 3)).toEqual(["Health", "Runners", "Computers"]);
    expect(screen.getByRole("tab", { name: "Computers" })).toHaveAttribute("aria-selected", "true");
  });
});

describe("AI endpoints", () => {
  it("is the endpoints alone: the AI key rules are no longer a tab here", async () => {
    window.history.replaceState(null, "", "/admin/ai-models");
    mock();
    wrap(<AdminAiModelsPage />);
    await screen.findByRole("heading", { name: "AI endpoints" });
    expect(screen.queryByRole("tab", { name: "Rules" })).toBeNull();
  });
});
