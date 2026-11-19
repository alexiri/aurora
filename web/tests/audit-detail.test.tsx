import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { AuditDetail } from "@/components/audit/audit-detail";
import type { AuditEntry } from "@/components/audit/audit-panel";

// D-373, C-1027: what a row recorded.

afterEach(cleanup);

const entry = (metadata: unknown, over: Partial<AuditEntry> = {}): AuditEntry => ({
  id: "e0000000-0000-4000-8000-000000000001",
  org_id: null,
  workspace_id: null,
  actor_kind: "instance_admin",
  actor_id: "a0000000-0000-4000-8000-000000000002",
  actor_label: "root@x.io",
  action: "instance_settings.update",
  target_kind: "instance_settings",
  target_id: "llm",
  outcome: "success",
  source: "dashboard",
  ip: "10.1.2.3",
  metadata,
  critical: true,
  created_at: "2026-11-18T10:00:00Z",
  ...over,
});
const show = (metadata: unknown, over: Partial<AuditEntry> = {}) => render(<AuditDetail entry={entry(metadata, over)} />);
const row = (name: string) => screen.getByText(name).closest("tr") as HTMLElement;

describe("settings changes", () => {
  it("shows a plain key before and after, and Added / Removed", () => {
    show({
      change: [
        { key: "limits.max", before: 5, after: 9 },
        { key: "llm.endpoints", entry_id: "eu", after: { url: "https://x" } },
        { key: "llm.endpoints", entry_id: "us", before: { url: "https://y" }, after: null },
      ],
    });
    expect(within(row("limits.max")).getByText("5")).toBeInTheDocument();
    expect(within(row("limits.max")).getByText("9")).toBeInTheDocument();
    expect(within(row("llm.endpoints › eu")).getByText("Added")).toBeInTheDocument();
    expect(within(row("llm.endpoints › us")).getByText("Removed")).toBeInTheDocument();
  });
  it("shows only the fields that differ for an object entry, and the whole entry on request", () => {
    show({
      change: [
        {
          key: "llm.endpoints",
          entry_id: "eu",
          before: { url: "https://a", model: "m1", keep: "same" },
          after: { url: "https://b", model: "m1", keep: "same" },
        },
      ],
    });
    const r = row("llm.endpoints › eu");
    expect(within(r).getByText("https://a")).toBeInTheDocument();
    expect(within(r).getByText("https://b")).toBeInTheDocument();
    expect(within(r).queryByText(/same/)).toBeNull();
    fireEvent.click(within(r).getByRole("button", { name: "Show whole entry" }));
    expect(r.textContent).toContain('"keep": "same"');
    expect(r.textContent).toContain('"url": "https://a"');
    expect(r.textContent).toContain('"url": "https://b"');
  });
  it("shows each secret marker verbatim", () => {
    show({
      change: [
        { key: "a.token", before: "(secret)", after: "(secret changed)" },
        { key: "b.token", before: "(secret)", after: "(secret removed)" },
      ],
    });
    expect(screen.getByText("(secret changed)")).toBeInTheDocument();
    expect(screen.getByText("(secret removed)")).toBeInTheDocument();
  });
  it("notes a value shortened when recorded", () => {
    show({ change: [{ key: "x.long", before: "abc", after: `${"y".repeat(20)}…` }] });
    expect(screen.getByText("Shortened when recorded")).toBeInTheDocument();
  });
  it("names the section of an import row", () => {
    show({ change: [{ key: "limits.max", section: "limits", before: 1, after: 2 }] });
    expect(screen.getByText("limits · limits.max")).toBeInTheDocument();
  });
});

describe("other recorded detail", () => {
  it("shows {from,to}, {before,after} and {old,new} pairs on one line", () => {
    show({ status: { from: "open", to: "closed" }, size: { before: 1, after: 2 } });
    const lines = screen.getAllByTestId("before-after").map((l) => l.textContent);
    expect(lines.join("|")).toMatch(/status:\s*open\s*→\s*to closed/);
    expect(lines.join("|")).toMatch(/size:\s*1\s*→\s*to 2/);
  });
  it("shows a top-level old and new pair as one line", () => {
    show({ old: "https://old", new: "https://new", source_id: "s1" });
    expect(screen.getByTestId("before-after").textContent).toMatch(/https:\/\/old\s*→\s*to https:\/\/new/);
    expect(screen.getByText("source id")).toBeInTheDocument();
  });
  it("shows a nested object as JSON and a scalar inline, labels with spaces", () => {
    show({ target_workspace: "w1", nested: { a: { b: 1 } } });
    expect(screen.getByText("target workspace")).toBeInTheDocument();
    expect(screen.getByText("w1")).toBeInTheDocument();
    const pre = document.querySelector("pre")!;
    expect(pre.textContent).toContain('"b": 1');
  });
  it("says so for empty metadata", () => {
    show({});
    expect(screen.getByText("No further detail recorded.")).toBeInTheDocument();
    show(null);
    expect(screen.getAllByText("No further detail recorded.")).toHaveLength(2);
  });
  it("renders markup as text", () => {
    show({ note: '<img src=x onerror="alert(1)">', change: [{ key: "k", before: "<b>x</b>", after: "<script>1</script>" }] });
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("b")).toBeNull();
    expect(document.querySelector("script")).toBeNull();
    expect(screen.getByText('<img src=x onerror="alert(1)">')).toBeInTheDocument();
    expect(screen.getByText("<b>x</b>")).toBeInTheDocument();
  });
});

describe("the row's own fields", () => {
  it("shows source, IP, actor, target, workspace and entry id; IP and workspace only when present", () => {
    show({}, { workspace_id: "b0000000-0000-4000-8000-000000000003" });
    for (const t of ["dashboard", "10.1.2.3", "instance_admin", "instance_settings"]) expect(screen.getAllByText(new RegExp(t)).length).toBeGreaterThan(0);
    expect(screen.getByText("Workspace")).toBeInTheDocument();
    cleanup();
    show({}, { ip: null, workspace_id: null, target_kind: null });
    expect(screen.queryByText("IP")).toBeNull();
    expect(screen.queryByText("Workspace")).toBeNull();
    expect(screen.queryByText("Target")).toBeNull();
  });
});
