import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { json, mockFetch } from "./support/api";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/addon-sources",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { AddonSourcesView } from "@/components/admin/addon-sources-view";
import { PluginsView } from "@/components/admin/plugins-view";
import { INTERVALS } from "@/lib/api/addon-sources";

const FP = "sha256:0123456789ab";
const NEW_FP = "sha256:ba9876543210";

function source(over: Record<string, unknown> = {}) {
  return {
    id: "s1",
    name: "CERN addons",
    url: "https://raw.githubusercontent.com/cern/addons/HEAD/aurora-addons.yaml",
    homepage: "https://aurora.cern.example",
    contact: null,
    publisher: "cern",
    key_fingerprint: FP,
    key_description: "CERN release key",
    update_interval: "1d",
    status: "ok",
    check_failed_at: null,
    check_failed_reason: null,
    new_key_fingerprint: null,
    last_checked_at: "2026-11-14T10:00:00Z",
    manifest_digest: "d".repeat(64),
    addons: [
      {
        id: "indigo_sso",
        name: "Indigo SSO",
        description: "Single sign-on",
        versions: [
          { version: "1.1.0", min_core: null, ingestable: true, not_ingestable_reason: null, installed_version: "1.0.0", update_available: true },
          { version: "1.0.0", min_core: null, ingestable: true, not_ingestable_reason: null, installed_version: "1.0.0", update_available: false },
        ],
      },
    ],
    addon_count: 1,
    removed_upstream: [{ package_id: "indigo_sso", version: "0.9.0" }],
    created_at: "2026-11-10T10:00:00Z",
    updated_at: "2026-11-14T10:00:00Z",
    ...over,
  };
}

describe("addon sources view (P5-ASRC, C-590..C-602)", () => {
  it("lists sources with key fingerprint, schedule, status and a pending-update highlight", async () => {
    mockFetch({ "GET /v1/admin/addon-sources": () => json(200, { items: [source()] }) });
    wrap(<AddonSourcesView />);
    expect(await screen.findByText("CERN addons")).toBeTruthy();
    expect(screen.getByLabelText("Signing key fingerprint").textContent).toBe(FP);
    expect(screen.getByText("Every day")).toBeTruthy();
    expect(screen.getByText("OK")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("1 addon has a newer version");
    // The per-source addon list: versions, installed, update, removed upstream.
    fireEvent.click(screen.getByRole("button", { name: "1 addon: CERN addons" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Indigo SSO")).toBeTruthy();
    expect(within(dialog).getByText("Update available")).toBeTruthy();
    expect(within(dialog).getByText("Removed upstream")).toBeTruthy();
    expect(within(dialog).getByText("indigo_sso 0.9.0")).toBeTruthy();
  });

  it("shows an empty state that offers the add flow", async () => {
    mockFetch({ "GET /v1/admin/addon-sources": () => json(200, { items: [] }) });
    wrap(<AddonSourcesView />);
    expect(await screen.findByText("No addon sources")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Add source" }).length).toBeGreaterThan(0);
  });

  it("adds a source only after the repository and its signing key were reviewed", async () => {
    let created: unknown = null;
    mockFetch({
      "GET /v1/admin/addon-sources": () => json(200, { items: [] }),
      "POST /v1/admin/addon-sources/preview": async (req) => {
        expect(await req.json()).toEqual({ url: "https://github.com/cern/addons" });
        return json(200, {
          url: "https://raw.githubusercontent.com/cern/addons/HEAD/aurora-addons.yaml",
          repo: { name: "CERN addons", description: "App packages", homepage: null, contact: "mailto:aurora@cern.example" },
          publisher: "cern",
          key_fingerprint: FP,
          key_description: "CERN release key",
          addons: source().addons,
          packages_new: 2,
        });
      },
      "POST /v1/admin/addon-sources": async (req) => {
        created = await req.json();
        return json(201, source());
      },
    });
    wrap(<AddonSourcesView />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Add source" }))[0]);
    const dialog = await screen.findByRole("dialog");
    // The registration button does not exist before the review.
    expect(within(dialog).queryByRole("button", { name: /Trust this key/ })).toBeNull();
    fireEvent.change(within(dialog).getByLabelText("Repository or manifest URL"), {
      target: { value: "https://github.com/cern/addons" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Review repository" }));
    // The fingerprint is the prominent item of the review, with what trusting it means.
    expect(await within(dialog).findByText("Signing key — check this fingerprint with the publisher")).toBeTruthy();
    expect(within(dialog).getByLabelText("Signing key fingerprint").textContent).toBe(FP);
    expect(within(dialog).getByText(/2 packages are new to this instance/)).toBeTruthy();
    expect(within(dialog).getByText(/Packages never activate on their own/)).toBeTruthy();
    // Every schedule value of the owner's enum is offered.
    const options = within(dialog).getAllByRole("option").map((o) => (o as HTMLOptionElement).value);
    expect(options).toEqual([...INTERVALS]);
    fireEvent.click(within(dialog).getByRole("button", { name: "Trust this key and add source" }));
    await waitFor(() =>
      expect(created).toEqual({
        url: "https://raw.githubusercontent.com/cern/addons/HEAD/aurora-addons.yaml",
        update_interval: "1d",
      }),
    );
  });

  it("surfaces the refusal of a repository (the review never opens)", async () => {
    mockFetch({
      "GET /v1/admin/addon-sources": () => json(200, { items: [] }),
      "POST /v1/admin/addon-sources/preview": () =>
        json(422, { error: { code: "addon_manifest_invalid", message: "The addon manifest is invalid." } }),
    });
    wrap(<AddonSourcesView />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Add source" }))[0]);
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Repository or manifest URL"), { target: { value: "https://x.example/r" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Review repository" }));
    expect((await within(dialog).findByRole("alert")).textContent).toContain("The addon manifest is invalid.");
    expect(within(dialog).queryByRole("button", { name: /Trust this key/ })).toBeNull();
  });

  it("checks one source now and reports the outcome", async () => {
    let calls = 0;
    mockFetch({
      "GET /v1/admin/addon-sources": () => json(200, { items: [source()] }),
      "POST /v1/admin/addon-sources/s1/check": () => {
        calls += 1;
        return json(200, {
          source_id: "s1",
          status: "updated",
          ingested: [{ id: "p2", package_id: "indigo_sso", version: "1.1.0", status: "pending_approval" }],
          failed_versions: [{ package_id: "indigo_sso", version: "1.2.0", reason: "digest_mismatch: the archive does not match" }],
          check_failed_reason: null,
        });
      },
    });
    wrap(<AddonSourcesView />);
    fireEvent.click(await screen.findByRole("button", { name: "Check now: CERN addons" }));
    await waitFor(() => expect(calls).toBe(1));
    const notices = await screen.findAllByRole("status");
    const text = notices.map((n) => n.textContent).join("\n");
    expect(text).toContain("CERN addons: 1 new version ready for review");
    expect(text).toContain("indigo_sso 1.2.0: digest_mismatch");
  });

  it("flags a changed signing key and re-confirms it with the fingerprint the admin saw", async () => {
    let confirmed: unknown = null;
    mockFetch({
      "GET /v1/admin/addon-sources": () =>
        json(200, { items: [source({ status: "signing_key_changed", new_key_fingerprint: NEW_FP })] }),
      "POST /v1/admin/addon-sources/s1/confirm-key": async (req) => {
        confirmed = await req.json();
        return json(200, source());
      },
    });
    wrap(<AddonSourcesView />);
    expect(await screen.findByText("Signing key changed")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Review key: CERN addons" }));
    const dialog = await screen.findByRole("dialog");
    // Old and new side by side.
    expect(within(dialog).getByLabelText("Old fingerprint").textContent).toBe(FP);
    expect(within(dialog).getByLabelText("New fingerprint").textContent).toBe(NEW_FP);
    fireEvent.click(within(dialog).getByRole("button", { name: "Trust the new key" }));
    await waitFor(() => expect(confirmed).toEqual({ new_key_fingerprint: NEW_FP }));
  });

  it("edits the schedule with exactly the owner's values", async () => {
    let patched: unknown = null;
    mockFetch({
      "GET /v1/admin/addon-sources": () => json(200, { items: [source()] }),
      "PATCH /v1/admin/addon-sources/s1": async (req) => {
        patched = await req.json();
        return json(200, source({ update_interval: "never" }));
      },
    });
    wrap(<AddonSourcesView />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit the schedule of CERN addons" }));
    const dialog = await screen.findByRole("dialog");
    const select = within(dialog).getByLabelText("Check for updates") as HTMLSelectElement;
    expect(Array.from(select.options).map((o) => o.value)).toEqual(["6h", "12h", "1d", "1w", "2w", "1mo", "never"]);
    expect(select.value).toBe("1d");
    fireEvent.change(select, { target: { value: "never" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save schedule" }));
    await waitFor(() => expect(patched).toEqual({ update_interval: "never" }));
  });

  it("states what removal does, and shows the refusal while packages are active", async () => {
    mockFetch({
      "GET /v1/admin/addon-sources": () => json(200, { items: [source()] }),
      "DELETE /v1/admin/addon-sources/s1": () =>
        json(409, { error: { code: "addon_source_active", message: "Packages of this source are active; disable or reject them first." } }),
    });
    wrap(<AddonSourcesView />);
    fireEvent.click(await screen.findByRole("button", { name: "Remove CERN addons" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("stay installed and keep working");
    expect(dialog.textContent).toContain("stops checking this repository");
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove source" }));
    expect((await within(dialog).findByRole("alert")).textContent).toContain("Packages of this source are active");
  });
});

describe("plugins list: provenance, update badges and the check-all button (C-600, C-602)", () => {
  const pkg = (over: Record<string, unknown>) => ({
    id: "p1",
    package_id: "indigo_sso",
    version: "1.0.0",
    contract: 1,
    publisher: "cern",
    digest: "d",
    signer: { kind: "trusted_key", name: "CERN addons signing key" },
    source: "addon",
    source_id: "s1",
    source_name: "CERN addons",
    origin: "addon",
    reaches: [],
    reaches_more: 0,
    apps: [{ app_id: "indigo", name: "Indigo SSO" }],
    update_available: true,
    status: "active",
    status_reason: null,
    approved_at: null,
    created_at: "2026-11-10T10:00:00Z",
    ...over,
  });

  it("shows the source column, the update badge and checks every source at once", async () => {
    let calls = 0;
    mockFetch({
      "GET /v1/admin/plugins": () =>
        json(200, {
          items: [
            pkg({}),
            pkg({ id: "p2", package_id: "github", version: "0.1.0", source: "builtin", origin: "aurora", apps: [{ app_id: "github", name: "GitHub" }], source_id: null, source_name: null, update_available: false }),
          ],
          next_cursor: null,
        }),
      "GET /v1/admin/addon-sources": () => json(200, { items: [source()] }),
      "POST /v1/admin/addon-sources/check-all": () => {
        calls += 1;
        return json(200, {
          items: [
            { source_id: "s1", status: "up_to_date", ingested: [], failed_versions: [], check_failed_reason: null },
          ],
        });
      },
    });
    wrap(<PluginsView />);
    expect(await screen.findByText("Indigo SSO")).toBeTruthy();
    expect(screen.getByText("GitHub")).toBeTruthy();
    expect(screen.getAllByText("Update available").length).toBeGreaterThan(0);
    fireEvent.click(await screen.findByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(calls).toBe(1));
    expect((await screen.findByLabelText("Update check results")).textContent).toContain("CERN addons: up to date");
  });

  it("has no check button while no addon source is registered", async () => {
    mockFetch({
      "GET /v1/admin/plugins": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/admin/addon-sources": () => json(200, { items: [] }),
    });
    wrap(<PluginsView />);
    expect(await screen.findByText("No apps yet")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Check for updates" })).toBeNull();
  });
});
