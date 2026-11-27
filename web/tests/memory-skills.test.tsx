import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { RevisionHistory } from "@/components/memory/memory-tab";
import { SizeMeter } from "@/components/memory/size-meter";
import { SkillsTab } from "@/components/skills/skills-tab";
import { canRedact, meter, type Memory, type MemoryRevision } from "@/lib/api/memory";
import { buildTree, validateSkillName, validateSkillPath } from "@/lib/api/skills";
import { wrap } from "./support/render";
import { json } from "./support/api";

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

const ann: Memory["added_by"] = { kind: "person", person: { id: "u1", display_name: "Ann" } };
const rev = (n: number, over: Partial<MemoryRevision> = {}): MemoryRevision => ({
  revision: n, author_kind: "user", author_label: "Ann", provenance: "dashboard", author: ann, via: "dashboard",
  redacted: false, content: `content ${n}`, created_at: "2026-01-01T00:00:00Z", ...over,
});
const memory: Memory = {
  key: "k", revision: 3, content: "c", index_line: "k: c", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
  locked: false, added_by: ann, changed_by: ann,
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
    const latest = await screen.findByRole("button", { name: "Remove content of #3" });
    expect((latest as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Content removed")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Restore #2" }));
    await waitFor(() => expect(restored).toEqual({ revision: 2 }));
    fireEvent.click(screen.getByRole("button", { name: "Remove content of #2" }));
    expect(redacted).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Confirm removing content of #2" }));
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
    // the server's own rules (P5-SKILLS-3)
    expect(validateSkillPath("a/b/c/d.sh")).toBeNull();
    expect(validateSkillPath("a/b/c/d/e.sh")).not.toBeNull();
    expect(validateSkillPath("my file.md")).not.toBeNull();
    expect(validateSkillPath("docs/Skill.md")).not.toBeNull();
    expect(validateSkillPath("a".repeat(201))).not.toBeNull();
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
    id: "s1", scope: "agent", tier: "agent", agent_id: "a1", org_id: "o1", name: "demo", description: "d", body: "b", enabled: true,
    files: [{ path: "big.txt", size: 39_999, executable: false }], total_size: 40_000, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
  };
  // (P5-SKILLS-3) The path rules and the executable bit of the Add file dialog are in `skill-form.test.tsx`.
  it("says in the editor when the instructions and files are over the total, and saves nothing", async () => {
    window.history.replaceState(null, "", "/");
    const fn = mockFetch({
      "GET /v1/agents/a1/skills/effective": () => json(200, { items: [{ skill, tier: "agent", enabled: true, shadows: [], off_for_agent: false, usage: null }], shadowed: [] }),
    });
    wrap(<SkillsTab agentId="a1" agentName="Dani" />);
    fireEvent.click(await screen.findByRole("button", { name: "demo" }));
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    expect(await screen.findByText("A skill's instructions and files can total at most 32,000 characters.")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Demo two" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    // `/v1/me` is fetched by its path; everything else is a Request.
    expect(fn.mock.calls.filter(([r]) => r instanceof Request && r.method !== "GET")).toEqual([]);
  });
});
