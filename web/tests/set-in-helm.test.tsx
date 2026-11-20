import { screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { json, mockFetch } from "./support/api";
import { wrap } from "./support/render";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/set-in-helm",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { SetInHelmView } from "@/components/admin/set-in-helm-view";
import { LockPill } from "@/components/ui/lock-pill";

describe("Set in Helm (P5-REFINE-18 #1)", () => {
  it("lists every locked value in words with its Helm key", async () => {
    mockFetch({
      "GET /v1/admin/settings": () =>
        json(200, {
          sections: [],
          locked: [
            { section: "terminal", key: "terminal.enabled", entry_id: null, helm_key: "terminal.enabled" },
            { section: "llm", key: "llm.endpoints", entry_id: "gw", helm_key: "llm.endpoints" },
          ],
        }),
      "GET /v1/admin/settings/terminal": () =>
        json(200, {
          section: "terminal",
          title: "Terminal",
          description: "",
          generation: 0,
          editable: true,
          fields: [{ key: "terminal.enabled", label: "Web terminal", help: "", kind: "bool", choices: null, default_display: "Off", unit: null, doc: null }],
          settings: [{ key: "terminal.enabled", value: true, source: "helm", locked: true, helm_key: "terminal.enabled", default: false, schema: { kind: "bool" }, list: null, id_field: null, entries: null, secret: null }],
        }),
      "GET /v1/admin/settings/llm": () =>
        json(200, { section: "llm", title: "AI endpoints", description: "", generation: 0, editable: true, fields: [], settings: [] }),
    });
    wrap(<SetInHelmView />);
    expect(await screen.findByText("These values are set when Aurora is installed. To change one, change the Helm values and upgrade.")).toBeTruthy();
    const list = await screen.findByRole("list", { name: "Values set in Helm" });
    const row = (await within(list).findByText("Web terminal")).closest("li")!;
    expect(within(row).getByText("On")).toBeTruthy();
    expect(within(row).getByText("terminal.enabled").tagName).toBe("CODE");
    expect(within(list).getAllByText(/Set in your Helm values/)).toHaveLength(2);
  });

  it("says so when nothing is set in Helm", async () => {
    mockFetch({ "GET /v1/admin/settings": () => json(200, { sections: [], locked: [] }) });
    wrap(<SetInHelmView />);
    expect(await screen.findByText(/Nothing is set in Helm/)).toBeTruthy();
  });

  it("every 'Set by the installation' pill links to Set in Helm; others do not", () => {
    wrap(
      <>
        <LockPill owner="Set by the installation" settingKey="terminal.enabled" />
        <LockPill owner="Set for you by an Aurora administrator" />
      </>,
    );
    expect(screen.getByRole("link", { name: /Set by the installation/ }).getAttribute("href")).toBe("/admin/set-in-helm#terminal.enabled");
    expect(screen.queryByRole("link", { name: /Set for you by/ })).toBeNull();
  });
});
