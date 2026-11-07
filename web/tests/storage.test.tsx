import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AdminStorageView } from "@/components/account/admin-storage-view";
import { OrgStorageView } from "@/components/org/storage-view";
import { uploadErrorMessage } from "@/lib/api/attachments";
import { ApiError } from "@/lib/api/client";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function mockFetch(routes: Record<string, unknown>) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const body = routes[`${req?.method ?? "GET"} ${url.pathname}`];
      return body === undefined ? json(404, { error: { code: "not_found", message: "nf" } }) : json(200, body);
    }),
  );
}

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const MIB = 1024 * 1024;

describe("storage pages (RFE-015)", () => {
  it("shows usage against the quota, the full notice and the breakdown", async () => {
    mockFetch({
      "GET /v1/orgs/o1/storage": {
        org_id: "o1", used_bytes: 10 * MIB, quota_bytes: 10 * MIB, notice: "full", files: 3, purged_files: 1,
        attachments_days: null, attachments_days_effective: null,
        by_workspace: [{ workspace_id: "w1", workspace_slug: "w", workspace_name: "Main", direction: "out", files: 3, bytes: 10 * MIB }],
      },
      "GET /v1/orgs/o1/retention": {
        conversations_days: null, conversations_days_max: null, attachments_days: null, attachments_days_max: null,
        attachment_quota_mib: null, attachment_quota_mib_effective: 10, attachment_quota_mib_max: null,
      },
    });
    wrap(<OrgStorageView orgId="o1" />);
    expect(await screen.findByRole("progressbar", { name: /attachment storage used/i })).toHaveAttribute("aria-valuenow", "100");
    expect(screen.getByRole("alert")).toHaveTextContent(/attachment storage is full/i);
    expect(screen.getByText("Main")).toBeInTheDocument();
    expect(screen.getByText("Sent by agents")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("form", { name: "Attachment limits" })).toBeInTheDocument());
  });

  it("lists instance totals, orgs and big tables", async () => {
    mockFetch({
      "GET /v1/admin/storage": {
        blob_bytes: 2 * MIB, blob_count: 2, attachment_bytes: 2 * MIB, attachment_files: 2, purged_files: 0,
        orgs: [{ org_id: "o1", slug: "acme", name: "Acme", attachment_bytes: 2 * MIB, files: 2, quota_bytes: null }],
        tables: [{ table: "blobs", bytes: 3 * MIB }],
      },
    });
    wrap(<AdminStorageView />);
    expect(await screen.findByText("Acme")).toBeInTheDocument();
    expect(screen.getByText("Unlimited")).toBeInTheDocument();
    expect(screen.getByText("blobs")).toBeInTheDocument();
  });

  it("explains a quota refusal on upload", () => {
    const err = ApiError.fromBody({ error: { code: "attachment_quota_exceeded", message: "full" } }, 422);
    expect(uploadErrorMessage(err)).toMatch(/attachment storage is full/i);
  });
});
