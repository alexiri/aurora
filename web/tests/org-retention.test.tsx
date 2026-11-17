import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { OrgRetentionForm } from "@/components/org/retention-form";

interface Call {
  method: string;
  path: string;
  body: unknown;
}
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function mock(routes: Record<string, () => Response>) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" ? "" : await req.clone().text();
      calls.push({ method: req.method, path: url.pathname, body: text ? JSON.parse(text) : undefined });
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

function wrap() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <OrgRetentionForm orgId="o1" />
    </QueryClientProvider>,
  );
}

const RETENTION = {
  conversations_days: 180,
  conversations_days_max: 365,
  attachments_days: null,
  attachments_days_max: 90,
  attachment_quota_mib: null,
  attachment_quota_mib_max: 102400,
  attachment_quota_mib_effective: 51200,
};

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("organization retention form (P5-IA-4, C-969)", () => {
  it("shows Administration's maximums above one form", async () => {
    mock({ "GET /v1/orgs/o1/retention": () => json(200, RETENTION) });
    wrap();
    const above = await screen.findByRole("region", { name: "Limits from above" });
    expect(above).toHaveTextContent("At most 365 days without activity");
    expect(above).toHaveTextContent("At most 90 days");
    expect(above).toHaveTextContent("At most 102400 MiB");
    expect(above).toHaveTextContent("Set by Administration");
    const form = screen.getByRole("form", { name: "Retention" });
    expect(form).toContainElement(screen.getByLabelText(/Delete conversations after/));
    expect(form).toContainElement(screen.getByLabelText(/Delete files after/));
    expect(form).toContainElement(screen.getByLabelText(/Storage for files/));
  });

  it("validates against the maximums and saves all three values in one request", async () => {
    const calls = mock({
      "GET /v1/orgs/o1/retention": () => json(200, RETENTION),
      "PUT /v1/orgs/o1/retention": () => json(200, { ...RETENTION, conversations_days: 30, attachments_days: 60, attachment_quota_mib: 2048 }),
    });
    wrap();
    const conv = (await screen.findByLabelText(/Delete conversations after/)) as HTMLInputElement;
    await waitFor(() => expect(conv.value).toBe("180"));
    const files = screen.getByLabelText(/Delete files after/);
    const quota = screen.getByLabelText(/Storage for files/);
    const save = screen.getByRole("button", { name: "Save retention" });

    fireEvent.change(conv, { target: { value: "3" } });
    fireEvent.click(save);
    expect(await screen.findByText(/from 7 to 3650/)).toBeInTheDocument();
    fireEvent.change(conv, { target: { value: "400" } });
    fireEvent.click(save);
    expect(await screen.findByText("Administration allows at most 365 days.")).toBeInTheDocument();
    fireEvent.change(conv, { target: { value: "30" } });
    fireEvent.change(files, { target: { value: "120" } });
    fireEvent.click(save);
    expect(await screen.findByText("Administration allows at most 90 days.")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "PUT")).toBe(false);

    fireEvent.change(files, { target: { value: "60" } });
    fireEvent.change(quota, { target: { value: "2048" } });
    fireEvent.click(save);
    expect(await screen.findByText("Retention saved.")).toBeInTheDocument();
    const puts = calls.filter((c) => c.method === "PUT");
    expect(puts).toHaveLength(1);
    expect(puts[0]!.body).toEqual({ conversations_days: 30, attachments_days: 60, attachment_quota_mib: 2048 });
  });

  it("empty values clear the windows, and Discard changes restores what is saved", async () => {
    const calls = mock({
      "GET /v1/orgs/o1/retention": () => json(200, { ...RETENTION, conversations_days_max: null, attachments_days_max: null, attachment_quota_mib_max: null }),
      "PUT /v1/orgs/o1/retention": () => json(200, RETENTION),
    });
    wrap();
    const conv = (await screen.findByLabelText(/Delete conversations after/)) as HTMLInputElement;
    await waitFor(() => expect(conv.value).toBe("180"));
    expect(screen.queryByRole("region", { name: "Limits from above" })).toBeNull();
    fireEvent.change(conv, { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(conv.value).toBe("180");
    fireEvent.change(conv, { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save retention" }));
    await waitFor(() => expect(calls.find((c) => c.method === "PUT")?.body).toEqual({ conversations_days: null, attachments_days: null, attachment_quota_mib: null }));
  });
});
