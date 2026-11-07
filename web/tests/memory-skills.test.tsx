import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { RevisionHistory } from "@/components/memory/memory-tab";
import { SizeMeter } from "@/components/memory/size-meter";
import { SkillsEditor } from "@/components/skills/skills-view";
import { canRedact, meter, type Memory, type MemoryRevision } from "@/lib/api/memory";
import { buildTree, validateSkillName, validateSkillPath } from "@/lib/api/skills";

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
type Handler = (req: Request) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${decodeURIComponent(url.pathname)}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const rev = (n: number, over: Partial<MemoryRevision> = {}): MemoryRevision => ({
  revision: n, author_kind: "user", author_label: "Ann", provenance: "dashboard", redacted: false,
  content: `content ${n}`, created_at: "2026-01-01T00:00:00Z", ...over,
});
const memory: Memory = {
  key: "k", revision: 3, content: "c", index_line: "k: c", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
};

describe("limits", () => {
  it("meter flags over-limit", () => {
    expect(meter(10, 100).over).toBe(false);
    expect(meter(101, 100)).toMatchObject({ over: true, ratio: 1 });
    wrap(<SizeMeter label="Body" used={101} max={100} />);
    expect(screen.getByRole("alert").textContent).toContain("over the limit");
  });
});

describe("revisions", () => {
  const revs = [rev(3), rev(2), rev(1, { redacted: true, content: null })];
  it("canRedact never allows the latest or redacted", () => {
    expect(canRedact(rev(3), 3)).toBe(false);
    expect(canRedact(rev(2), 3)).toBe(true);
    expect(canRedact(revs[2], 3)).toBe(false);
  });
  it("disables redact on latest, restores a revision, redacts with confirmation", async () => {
    let restored: unknown = null;
    let redacted = false;
    mockFetch({
      "GET /v1/agents/a1/memories/k/revisions": () => json(200, { items: revs, next_cursor: null }),
      "POST /v1/agents/a1/memories/k/restore": async (r) => { restored = await r.json(); return json(200, memory); },
      "POST /v1/agents/a1/memories/k/revisions/2/redact": () => { redacted = true; return json(204, null); },
    });
    wrap(<RevisionHistory agentId="a1" memory={memory} />);
    const latest = await screen.findByRole("button", { name: "Redact #3" });
    expect((latest as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Redacted")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Restore #2" }));
    await waitFor(() => expect(restored).toEqual({ revision: 2 }));
    fireEvent.click(screen.getByRole("button", { name: "Redact #2" }));
    expect(redacted).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Confirm redact #2" }));
    await waitFor(() => expect(redacted).toBe(true));
  });
});

describe("skill paths", () => {
  it("rejects traversal and absolute paths", () => {
    expect(validateSkillPath("../x")).not.toBeNull();
    expect(validateSkillPath("a/../x")).not.toBeNull();
    expect(validateSkillPath("/etc/x")).not.toBeNull();
    expect(validateSkillPath("a//b")).not.toBeNull();
    expect(validateSkillPath("scripts/run.sh")).toBeNull();
  });
  it("validates names and builds a tree", () => {
    expect(validateSkillName("aurora-x")).not.toBeNull();
    expect(validateSkillName("Bad")).not.toBeNull();
    expect(validateSkillName("good-name")).toBeNull();
    const t = buildTree([{ path: "b.txt", size: 1, executable: false }, { path: "s/run.sh", size: 2, executable: true }]);
    expect(t.map((n) => n.name)).toEqual(["s", "b.txt"]);
  });
});

describe("skill files", () => {
  const skill = {
    id: "s1", scope: "agent", agent_id: "a1", org_id: "o1", name: "demo", description: "d", body: "b", enabled: true,
    files: [], total_size: 40_000, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
  };
  it("rejects bad path client-side and sends the executable bit on upload", async () => {
    let query = "";
    let body = "";
    const fn = mockFetch({
      "GET /v1/agents/a1/skills": () => json(200, { items: [skill], next_cursor: null }),
      "PUT /v1/skills/s1/files/scripts/run.sh": async (r) => {
        query = new URL(r.url).search;
        body = await r.text();
        return json(200, skill);
      },
    });
    wrap(<SkillsEditor scope={{ kind: "agent", id: "a1" }} />);
    fireEvent.click(await screen.findByRole("button", { name: /demo/ }));
    expect(await screen.findByText(/Total size: 40,000 \/ 32,000/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("File path"), { target: { value: "../evil" } });
    fireEvent.click(screen.getByRole("button", { name: "Save file" }));
    expect((await screen.findAllByRole("alert")).some((a) => a.textContent?.includes(".."))).toBe(true);
    expect(fn.mock.calls.some(([r]) => (r as Request).method === "PUT")).toBe(false);

    fireEvent.change(screen.getByLabelText("File path"), { target: { value: "scripts/run.sh" } });
    fireEvent.change(screen.getByLabelText("Or type text content"), { target: { value: "echo hi" } });
    fireEvent.click(screen.getByLabelText("Executable"));
    fireEvent.click(screen.getByRole("button", { name: "Save file" }));
        await waitFor(() => expect(query).toBe("?executable=true"));
    expect(body).toBe("echo hi");
  });
});
