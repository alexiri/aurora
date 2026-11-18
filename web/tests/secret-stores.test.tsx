import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { wrap } from "./support/render";
import { json, mockFetch } from "./support/api";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/secret-stores",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { SecretStoresView } from "@/components/admin/secret-stores-view";

const health = { ok: true, checked_at: "2026-11-15T10:00:00Z", error_code: null };
const noMigration = {
  state: "none",
  target: null,
  started_at: null,
  finished_at: null,
  started_by: null,
  per_purpose: [],
  error: null,
  running_elsewhere: false,
};

function store(over: Record<string, unknown>) {
  return {
    id: "inline",
    kind: "inline",
    source: "builtin",
    helm_key: null,
    settings: {},
    references: false,
    default: false,
    health,
    error: null,
    managed_values: 0,
    reference_values: 0,
    reference_scopes: [],
    ...over,
  };
}

function status(over: Record<string, unknown> = {}) {
  return {
    default_store: { value: "inline", source: "default" },
    effective_default: "inline",
    generation: 4,
    migration: noMigration,
    stores: [
      store({ default: true, managed_values: 7 }),
      store({
        id: "helmvault",
        kind: "hashicorp-vault",
        source: "helm",
        helm_key: "vault.*",
        settings: { address: "https://h.example.org:8200", auth: { method: "kubernetes", role: "r" } },
        references: true,
      }),
      store({
        id: "mine",
        kind: "hashicorp-vault",
        source: "dashboard",
        settings: {
          address: "https://v.example.org:8200",
          kv_mount: "kv",
          auth: { method: "approle", role_id: "r1", secret_id: { $secret: { set: true } } },
        },
        references: true,
        health: { ok: false, checked_at: "2026-11-15T10:00:00Z", error_code: "unavailable" },
        reference_scopes: [{ org_id: "0197-aaaa", org_slug: "acme", paths: ["teams/acme/*"] }],
      }),
    ],
    ...over,
  };
}

describe("Admin › Secret stores", () => {
  it("shows a Helm store read-only and a dashboard store with its controls", async () => {
    mockFetch({ "GET /v1/admin/secret-stores": () => json(200, status()) });
    wrap(<SecretStoresView />);
    const list = await screen.findByRole("list", { name: "Secret stores" });
    const rows = within(list).getAllByRole("listitem");
    const helm = rows.find((r) => within(r).queryByText("helmvault"))!;
    expect(within(helm).getByText("Configured in Helm")).toBeTruthy();
    expect(within(helm).getByText(/Change it in the values file/)).toBeTruthy();
    expect(within(helm).queryByRole("button")).toBeNull();
    const mine = rows.find((r) => within(r).queryByText("mine"))!;
    expect(within(mine).getByRole("button", { name: "Edit mine" })).toBeTruthy();
    expect(within(mine).getByRole("button", { name: "Remove mine" })).toBeTruthy();
    expect(within(mine).getByText("Not answering")).toBeTruthy();
    const scopes = within(mine).getByRole("list", { name: "Reference scopes of mine" });
    expect(scopes.textContent).toContain("acme");
    expect(scopes.textContent).toContain("teams/acme/*");
    // The sealed secret id is never rendered.
    expect(document.body.textContent).not.toMatch(/secret-id-value/);
  });

  it("locks the default store when Helm sets it", async () => {
    mockFetch({
      "GET /v1/admin/secret-stores": () =>
        json(200, status({ default_store: { value: "helmvault", source: "helm" }, effective_default: "helmvault" })),
    });
    wrap(<SecretStoresView />);
    expect(await screen.findByText(/vault\.makeDefault/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Make default" })).toBeNull();
  });

  it("makes another store the default with the generation it loaded", async () => {
    let body: unknown = null;
    mockFetch({
      "GET /v1/admin/secret-stores": () => json(200, status()),
      "PUT /v1/admin/secret-store-default": async (req) => {
        body = await req.json();
        return json(200, status({ default_store: { value: "helmvault", source: "dashboard" } }));
      },
    });
    wrap(<SecretStoresView />);
    const select = (await screen.findByLabelText("Store for new values")) as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "helmvault" } });
    fireEvent.click(screen.getByRole("button", { name: "Make default" }));
    await waitFor(() => expect(body).toEqual({ generation: 4, store_id: "helmvault" }));
  });

  it("tests a connection before saving and shows each step", async () => {
    let tested: { id: string; store: Record<string, unknown> } | null = null;
    mockFetch({
      "GET /v1/admin/secret-stores": () => json(200, status()),
      "POST /v1/admin/secret-store-test": async (req) => {
        tested = (await req.json()) as typeof tested;
        return json(200, {
          ok: false,
          checked_at: "2026-11-15T10:00:00Z",
          steps: [
            { name: "configuration", ok: true, error_code: null },
            { name: "authenticate", ok: false, error_code: "unreachable" },
          ],
        });
      },
    });
    wrap(<SecretStoresView />);
    fireEvent.click(await screen.findByRole("button", { name: "Add a Vault store" }));
    fireEvent.change(screen.getByLabelText("Store id"), { target: { value: "prod" } });
    fireEvent.change(screen.getByLabelText("Address"), { target: { value: "https://vault.example.org:8200" } });
    // D-307: a dashboard store is AppRole only (the Kubernetes login is for Helm stores).
    fireEvent.change(screen.getByLabelText("Role id"), { target: { value: "aurora" } });
    fireEvent.change(screen.getByLabelText("Secret id"), { target: { value: "s3cret" } });
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
    const out = await screen.findByRole("status", { name: "Connection test result" });
    expect(within(out).getByText("The connection failed.")).toBeTruthy();
    expect(within(out).getByText(/Signed in to the store — the store did not answer/)).toBeTruthy();
    expect(tested).toEqual({
      id: "prod",
      store: {
        kind: "hashicorp-vault",
        address: "https://vault.example.org:8200",
        auth: { method: "approle", role_id: "aurora", secret_id: { $secret: "s3cret" } },
      },
    });
  });

  it("sends an AppRole secret id as a write-only value and keeps it when left empty", async () => {
    let body: { store: Record<string, unknown> } | null = null;
    mockFetch({
      "GET /v1/admin/secret-stores": () => json(200, status()),
      "PUT /v1/admin/secret-stores/mine": async (req) => {
        body = (await req.json()) as typeof body;
        return json(200, status());
      },
    });
    wrap(<SecretStoresView />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit mine" }));
    const secret = screen.getByLabelText("Secret id") as HTMLInputElement;
    expect(secret.type).toBe("password");
    expect(secret.value).toBe("");
    fireEvent.click(screen.getByRole("button", { name: "Save store" }));
    await waitFor(() => expect(body).not.toBeNull());
    expect((body!.store.auth as Record<string, unknown>).secret_id).toEqual({ $secret: "keep" });
    expect(body!.store.reference_scopes).toEqual([{ org_id: "0197-aaaa", paths: ["teams/acme/*"] }]);
  });

  it("explains a stale write (no re-authentication step, FIX-157)", async () => {
    mockFetch({
      "GET /v1/admin/secret-stores": () => json(200, status()),
      "PUT /v1/admin/secret-store-default": () =>
        json(409, { error: { code: "stale_revision", message: "changed" } }),
    });
    wrap(<SecretStoresView />);
    fireEvent.change(await screen.findByLabelText("Store for new values"), { target: { value: "helmvault" } });
    fireEvent.click(screen.getByRole("button", { name: "Make default" }));
    expect(await screen.findByText(/Someone changed the secret stores meanwhile/)).toBeTruthy();
  });

  it("previews what would move, then migrates and shows the progress", async () => {
    let started: unknown = null;
    let state = "none";
    mockFetch({
      "GET /v1/admin/secret-stores": () =>
        json(
          200,
          status({
            migration:
              state === "running"
                ? {
                    ...noMigration,
                    state: "running",
                    target: "helmvault",
                    per_purpose: [{ purpose: "secret.value", moved: 3, skipped: 0, failed: 0, already: 0, total: 7 }],
                  }
                : noMigration,
          }),
        ),
      "POST /v1/admin/secret-stores/migrations": async (req) => {
        const b = (await req.json()) as { dry_run: boolean };
        if (b.dry_run) {
          return json(200, {
            ...noMigration,
            state: "dry_run",
            target: "helmvault",
            per_purpose: [{ purpose: "secret.value", moved: 7, skipped: 0, failed: 0, already: 0, total: 7 }],
          });
        }
        started = b;
        state = "running";
        return json(202, { ...noMigration, state: "running", target: "helmvault" });
      },
    });
    wrap(<SecretStoresView />);
    fireEvent.change(await screen.findByLabelText("Move values to"), { target: { value: "helmvault" } });
    fireEvent.click(screen.getByRole("button", { name: "Preview what would move" }));
    const table = await screen.findByRole("table", { name: "What would move to helmvault" });
    expect(within(table).getByText("secret.value")).toBeTruthy();
    expect(within(table).getByText("7")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Migrate to helmvault…" }));
    fireEvent.click(await screen.findByRole("button", { name: "Start migration" }));
    await waitFor(() => expect(started).toEqual({ target: "helmvault", dry_run: false }));
    const progress = await screen.findByRole("table", { name: "Migration progress" });
    expect(within(progress).getByText("3")).toBeTruthy();
    expect(screen.getByText(/Moving values to helmvault/)).toBeTruthy();
  });
});
