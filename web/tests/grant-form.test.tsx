import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { GiveAccessPanel, type GrantSubject } from "@/components/credentials/give-access";
import { GrantForm } from "@/components/grants/grant-form";
import { ScopeGroups } from "@/components/grants/scope-groups";
import { initialGrantValue, splitByScopes, type GrantValue, type Tool } from "@/lib/grant-editor";

afterEach(cleanup);

const tool = (id: string, name: string, group: "read" | "write", over: Partial<Tool> = {}): Tool => ({
  id, name, group, host: "indico.example.org", path: `/${id}`, methods: group === "read" ? ["GET"] : ["POST"], connect_methods: [], scopes: [], wildcard: false, ...over,
});
const tools: Tool[] = [
  tool("get_event", "Read an event", "read"),
  tool("list_events", "List events", "read"),
  tool("book_room", "Book a room", "write"),
];

/** The real form in a tiny harness that holds the value, as a caller does. */
function Harness({ customOnly = false, blockUntooled = false, onValue }: { customOnly?: boolean; blockUntooled?: boolean; onValue?: (v: GrantValue) => void }) {
  const [value, setValue] = useState(() => initialGrantValue(tools, null, customOnly));
  return (
    <GrantForm
      appName="Indico"
      tools={tools}
      inert={["removed_tool"]}
      customOnly={customOnly}
      blockUntooled={blockUntooled}
      value={value}
      onChange={(v) => {
        setValue(v);
        onValue?.(v);
      }}
    />
  );
}
const radio = (group: string, option: string) => within(screen.getByRole("radiogroup", { name: group })).getByLabelText(option) as HTMLInputElement;

describe("tools the token cannot serve (FIX-136, D-241)", () => {
  const scoped = [
    tool("get_event", "Read an event", "read", { scopes: ["read:legacy_api"] }),
    tool("search_rooms", "Search rooms", "read", { scopes: ["read:everything"] }),
    tool("create_booking", "Book a room (web API)", "write", { scopes: ["full:everything"] }),
    tool("get_user", "Get the user", "read"),
  ];

  it("splits by the connection's known scopes: a tool with no requirement, or with unknown scopes, is never hidden", () => {
    const ids = (ts: Tool[]) => ts.map((t) => t.id);
    const s = splitByScopes(scoped, ["read:legacy_api"]);
    expect(ids(s.usable)).toEqual(["get_event", "get_user"]);
    expect(ids(s.unavailable)).toEqual(["search_rooms", "create_booking"]);
    // any-of: one matching scope is enough
    expect(ids(splitByScopes([tool("x", "X", "read", { scopes: ["a", "b"] })], ["b"]).usable)).toEqual(["x"]);
    // unknown scopes (empty / null) hide nothing
    expect(splitByScopes(scoped, []).unavailable).toEqual([]);
    expect(splitByScopes(scoped, null).unavailable).toEqual([]);
  });

  it("lists unavailable tools collapsed with the reason, and they are not in the permission groups", () => {
    const { usable, unavailable } = splitByScopes(scoped, ["read:legacy_api"]);
    render(
      <GrantForm
        appName="Indico"
        tools={usable}
        unavailable={unavailable}
        customOnly
        blockUntooled={false}
        value={initialGrantValue(usable, null, true)}
        onChange={() => {}}
      />,
    );
    const box = screen.getByText(/Not available with this connection's token \(2\)/).closest("details") as HTMLDetailsElement;
    expect(box.open).toBe(false);
    expect(within(box).getByText("Search rooms")).toBeTruthy();
    expect(within(box).getByText("Needs read:everything")).toBeTruthy();
    expect(within(box).getByText("Needs full:everything")).toBeTruthy();
    // Not selectable: no access control exists for them.
    expect(screen.queryByRole("radiogroup", { name: "Access for Search rooms" })).toBeNull();
    expect(screen.getByRole("radiogroup", { name: "Access for Read an event" })).toBeTruthy();
  });
});

describe("GrantForm (controlled)", () => {
  it("starts on Full access without a tool list, and Custom shows Read and Write groups", () => {
    render(<Harness />);
    expect(radio("Access mode", "Full access").checked).toBe(true);
    expect(screen.queryByRole("region", { name: "Read tools" })).toBeNull();
    fireEvent.click(radio("Access mode", "Custom"));
    expect(screen.getByRole("region", { name: "Read tools" })).toBeTruthy();
    expect(screen.getByRole("region", { name: "Write tools" })).toBeTruthy();
    expect(screen.getByText("Allow or ask at least one tool to save.")).toBeTruthy();
  });

  it("sets a whole group in one click and states all / some / none through the group control", () => {
    let last: GrantValue | undefined;
    render(<Harness onValue={(v) => (last = v)} />);
    fireEvent.click(radio("Access mode", "Custom"));
    // none: every row blocked, the group control shows Block
    expect(radio("Set all read tools", "Block").checked).toBe(true);
    fireEvent.click(radio("Set all read tools", "Allow"));
    expect(last?.decisions).toMatchObject({ get_event: "allow", list_events: "allow", book_room: "block" });
    expect(radio("Set all read tools", "Allow").checked).toBe(true); // all
    fireEvent.click(radio("Access for List events", "Ask"));
    // some: no option of the group control is checked and the heading says so for assistive technology
    for (const o of ["Allow", "Ask", "Block"]) expect(radio("Set all read tools", o).checked).toBe(false);
    expect(within(screen.getByRole("region", { name: "Read tools" })).getByText("(mixed)")).toBeTruthy();
    expect(radio("Access for Read an event", "Allow").checked).toBe(true);
    expect(screen.queryByText("Allow or ask at least one tool to save.")).toBeNull();
  });

  it("asks for a timeout only with Ask tools and flags a bad one", () => {
    render(<Harness />);
    fireEvent.click(radio("Access mode", "Custom"));
    expect(screen.queryByLabelText("Approval timeout (seconds)")).toBeNull();
    fireEvent.click(radio("Access for Book a room", "Ask"));
    fireEvent.change(screen.getByLabelText("Approval timeout (seconds)"), { target: { value: "5" } });
    expect(screen.getByText(/Enter a whole number of seconds from 60 to 14400/)).toBeTruthy();
  });

  it("lists inert tools with a Block control", () => {
    render(<Harness />);
    fireEvent.click(radio("Access mode", "Custom"));
    const inert = screen.getByRole("region", { name: "Inert tools" });
    expect(within(inert).getByText("removed_tool")).toBeTruthy();
    expect(radio("Access for removed_tool", "Block").checked).toBe(true);
  });

  it("custom_only: no mode switch, a note, and every tool starts blocked; block_untooled adds its note", () => {
    render(<Harness customOnly blockUntooled />);
    expect(screen.getByText(/This app only supports custom access/)).toBeTruthy();
    expect(screen.queryByRole("radiogroup", { name: "Access mode" })).toBeNull();
    expect(radio("Access for Read an event", "Block").checked).toBe(true);
    expect(screen.getByText("Requests that are not in the tool list are always blocked.")).toBeTruthy();
  });

  it("group controls are reachable by keyboard semantics: native radios sharing a name", () => {
    render(<Harness customOnly />);
    const g = screen.getByRole("radiogroup", { name: "Set all write tools" });
    const names = new Set([...g.querySelectorAll("input[type=radio]")].map((i) => (i as HTMLInputElement).name));
    expect(names.size).toBe(1);
  });
});

describe("ScopeGroups", () => {
  const scopes = [
    { scope: "repo:read", name: "Read repositories", access: "read" },
    { scope: "issues:read", name: "Read issues", access: "read" },
    { scope: "repo:write", name: "Write repositories", access: "write" },
  ];
  function ScopeHarness({ onChange }: { onChange: (s: string[]) => void }) {
    const [sel, setSel] = useState<string[]>(["repo:read"]);
    return <ScopeGroups scopes={scopes} selected={sel} onChange={(n) => { setSel(n); onChange(n); }} />;
  }

  it("groups by access with On / Off rows and a group-wide control", () => {
    const seen: string[][] = [];
    render(<ScopeHarness onChange={(s) => seen.push(s)} />);
    expect(screen.getByRole("region", { name: "Read scopes" })).toBeTruthy();
    expect(screen.getByRole("region", { name: "Write scopes" })).toBeTruthy();
    expect(screen.getByText("1 of 2 on")).toBeTruthy();
    expect(radio("Read repositories", "On").checked).toBe(true);
    fireEvent.click(radio("Set all read scopes", "On"));
    expect(seen.at(-1)).toEqual(["repo:read", "issues:read"]);
    fireEvent.click(radio("Write repositories", "On"));
    expect(seen.at(-1)).toEqual(["repo:read", "issues:read", "repo:write"]);
    fireEvent.click(radio("Set all read scopes", "Off"));
    expect(seen.at(-1)).toEqual(["repo:write"]);
  });
});

// ---- Give agents access ----------------------------------------------------------------------------------------------
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
let calls: { key: string; body: unknown }[];
let routes: Record<string, (body: unknown) => Response>;
beforeEach(() => {
  calls = [];
  routes = {};
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const body = text ? JSON.parse(text) : undefined;
      const key = `${req.method} ${url.pathname}`;
      calls.push({ key, body });
      const h = routes[key];
      return h ? h(body) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});
afterEach(() => vi.unstubAllGlobals());

describe("Give agents access with the shared form", () => {
  const subject: GrantSubject = {
    target: { kind: "connection", id: "c1" },
    scope: { kind: "workspace", id: "w1" },
    orgId: "o1",
    appId: "indico",
    appWorkspaceId: "w1",
  };
  const agent = (id: string, name: string) => ({ id, name, handle: name.toLowerCase(), kind: "hosted", created_at: "", updated_at: "" });
  beforeEach(() => {
    routes["GET /v1/workspaces/w1/agents"] = () => json(200, { items: [agent("a1", "Daneel"), agent("a2", "Giskard")], next_cursor: null });
    routes["GET /v1/apps/indico"] = () => json(200, { app_id: "indico", name: "Indico", tools, app_restrictions: { custom_only: true, block_untooled: false } });
    routes["POST /v1/connections/c1/grants"] = () => json(200, { items: [{ agent_id: "a1", grant: {}, status: "created" }] });
  });
  const panel = (s: GrantSubject = subject) => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <GiveAccessPanel subject={s} onDismiss={() => {}} />
      </QueryClientProvider>,
    );
  };

  it("custom_only (learned from the app): Allow all Read in one click sends mode, allow_tools, ask_tools, ask_timeout_secs", async () => {
    routes["GET /v1/connections/c1/grants"] = () => json(200, { items: [] });
    panel();
    fireEvent.click(await screen.findByRole("checkbox", { name: "Give Daneel access" }));
    await screen.findByRole("radiogroup", { name: "Set all read tools" });
    fireEvent.click(radio("Set all read tools", "Allow"));
    fireEvent.click(radio("Access for Book a room", "Ask"));
    fireEvent.change(screen.getByLabelText("Approval timeout (seconds)"), { target: { value: "900" } });
    fireEvent.click(screen.getByRole("button", { name: "Give Daneel access" })); // (C-925) the button names who
    await screen.findByText("Access given to 1 agent.");
    expect(calls.find((c) => c.key === "POST /v1/connections/c1/grants")?.body).toEqual({
      agent_ids: ["a1"], mode: "custom", allow_tools: ["get_event", "list_events"], ask_tools: ["book_room"], ask_timeout_secs: 900,
    });
  });

  it("a normal app starts on Full access and sends mode full", async () => {
    routes["GET /v1/apps/indico"] = () => json(200, { app_id: "indico", name: "Indico", tools });
    routes["GET /v1/connections/c1/grants"] = () => json(200, { items: [] });
    panel();
    fireEvent.click(await screen.findByRole("checkbox", { name: "Give Daneel access" }));
    await waitFor(() => expect(radio("Access mode", "Full access").checked).toBe(true));
    fireEvent.click(screen.getByRole("button", { name: "Give Daneel access" }));
    await screen.findByText("Access given to 1 agent.");
    expect(calls.find((c) => c.key === "POST /v1/connections/c1/grants")?.body).toEqual({ agent_ids: ["a1"], mode: "full" });
  });

  it("an agent that already has a grant is 'Has access' with an Edit that opens the agent's own editor", async () => {
    routes["GET /v1/connections/c1/grants"] = () => json(200, { items: [{ agent_id: "a2", agent_name: "Giskard", workspace_id: "w1", workspace_name: "Ops" }] });
    routes["GET /v1/workspaces/w1/connections"] = () =>
      json(200, {
        items: [{ app_id: "indico", app_version: "1", created_at: "", id: "c1", injecting: [], label: "CERN Indico", metadata: {}, missing_scopes: [], org_id: "o1", package_id: "indico", scope: "workspace", scopes: [], status: "active", updated_at: "", workspace_id: "w1" }],
        next_cursor: null,
      });
    routes["GET /v1/agents/a2/grants"] = () =>
      json(200, {
        secrets: [],
        connections: [{ id: "g1", agent_id: "a2", connection_id: "c1", created_at: "", mode: "custom", allow_tools: ["get_event"], ask_tools: [], ask_timeout_secs: null }],
        converted_by_app: [],
      });
    panel();
    expect(await screen.findByText("Has access")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Edit access for Giskard" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Access to CERN Indico" })).toBeTruthy();
    expect(radio("Access for Read an event", "Allow").checked).toBe(true);
    expect(radio("Access for List events", "Block").checked).toBe(true);
  });
});

// ---- the guard --------------------------------------------------------------------------------------------------------
describe("one permissions editor (FIX-147, D-233)", () => {
  function sources(dir: string): string[] {
    return readdirSync(dir).flatMap((n) => {
      const p = join(dir, n);
      return statSync(p).isDirectory() ? sources(p) : /\.tsx$/.test(n) ? [p] : [];
    });
  }

  it("no checkbox list renders an app's tools or scopes outside components/grants/**", () => {
    const root = join(process.cwd(), "src");
    const offenders = sources(root).filter((f) => {
      if (f.includes(`${join("components", "grants")}`)) return false;
      const s = readFileSync(f, "utf8");
      return /type="checkbox"/.test(s) && /\b(?:tools|scopes|method\.scopes|app\.tools|declared_scopes)\.map\(/.test(s);
    });
    expect(offenders).toEqual([]);
  });
});
