import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  usePathname: () => "/admin/internet-access",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  useParams: () => ({}),
}));

import { ChoiceListEditor } from "@/components/admin/settings/settings-field-controls";
import { AgentsSettingsView } from "@/components/admin/settings/operational-settings-views";

// FIX-172 (D-374, C-1028): harnesses are ticked and picked by name, never typed.

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- request bodies are inspected loosely
type Json = Record<string, any>;
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const CHOICES = [
  { value: "claude-code", label: "Claude Code", description: "Anthropic, Bedrock and Vertex directly." },
  { value: "codex", label: "OpenAI Codex", description: "Endpoints that serve the OpenAI Responses API." },
  { value: "pi", label: "Pi", description: "Pi sentence." },
  { value: "goose", label: "goose", description: "goose sentence." },
  { value: "opencode", label: "OpenCode", description: "OpenCode sentence." },
];
const base = { locked: false, helm_key: "config.x", list: null, id_field: null, entries: null, secret: null, source: "default" };
const sch = (kind: string, o: Json = {}) => ({ kind, min: null, max: null, max_len: null, options: null, item_kind: null, ...o });
const field = (key: string, kind: string, label: string, choices: unknown = null) => ({
  key, label, help: `Help ${key}.`, kind, choices, default_display: "Claude Code", unit: null, doc: null,
});

function section(opts: {
  enabled: string[];
  def: string;
  offered: string[] | null;
  lockedEnabled?: boolean;
  lockedDefault?: boolean;
  runners?: Json[];
}): Json {
  return {
    section: "agents", title: "Agents & sandboxes", description: "d", generation: 3, updated_at: null, updated_by_label: null,
    editable: true, current: null,
    fields: [
      field("hosted.default_harness", "enum", "Agent program new agents use", CHOICES),
      field("hosted.enabled_harnesses", "list", "Agent programs agents may run", CHOICES),
    ],
    extra: { runners: opts.runners ?? [], offered_harnesses: opts.offered },
    settings: [
      { ...base, key: "hosted.default_harness", value: opts.def, default: "claude-code", locked: !!opts.lockedDefault, source: opts.lockedDefault ? "helm" : "default", schema: sch("string", { max_len: 64 }) },
      { ...base, key: "hosted.enabled_harnesses", value: opts.enabled, default: ["claude-code"], locked: !!opts.lockedEnabled, source: opts.lockedEnabled ? "helm" : "default", schema: sch("list", { item_kind: "string" }) },
    ],
  };
}

function show(s: Json) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = typeof input === "string" ? new Request(new URL(input, "http://localhost")) : (input as Request);
      const url = new URL(req.url, "http://localhost");
      if (req.method === "GET" && url.pathname === "/v1/admin/settings/agents") return json(200, s);
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <AgentsSettingsView />
    </QueryClientProvider>,
  );
}

const group = () => screen.findByRole("group", { name: "Programs agents may run" });
const picker = () => screen.getByLabelText("Program new agents use") as HTMLSelectElement;

describe("Programs agents may run", () => {
  it("shows the display names, one checkbox each, with the sentence as a hint", async () => {
    show(section({ enabled: ["claude-code"], def: "claude-code", offered: ["claude-code", "codex", "goose", "pi", "opencode"] }));
    const g = await group();
    expect(within(g).getAllByRole("checkbox").map((c) => (c as HTMLInputElement).labels?.[0]?.textContent)).toEqual(
      ["Claude Code", "OpenAI Codex", "Pi", "goose", "OpenCode"],
    );
    expect(within(g).getByLabelText("Claude Code")).toBeChecked();
    expect(g).toHaveTextContent("Endpoints that serve the OpenAI Responses API.");
    expect(g).not.toHaveTextContent("claude-code");
  });

  it("disables an unticked program no runner offers, and warns on a ticked one", async () => {
    show(section({ enabled: ["claude-code", "pi"], def: "claude-code", offered: ["claude-code", "codex"] }));
    const g = await group();
    expect(within(g).getByLabelText("OpenAI Codex")).not.toBeDisabled();
    // Unticked and not offered: cannot be ticked.
    expect(within(g).getByLabelText("goose")).toBeDisabled();
    // Ticked but not offered: stays enabled so it can be unticked, with the warning.
    expect(within(g).getByLabelText("Pi")).not.toBeDisabled();
    expect(within(g).getAllByText("No registered runner offers this.")).toHaveLength(3);
    expect(document.getElementById("harness-pi-hint")).toHaveTextContent("No registered runner offers this.");
  });

  it("with no runner registered shows a notice and disables nothing", async () => {
    show(section({ enabled: ["claude-code"], def: "claude-code", offered: null }));
    const g = await group();
    expect(within(g).getByText(/No runner is registered yet/)).toBeInTheDocument();
    for (const c of within(g).getAllByRole("checkbox")) expect(c).not.toBeDisabled();
  });

  it("keeps a value that is not a choice, shows it, and writes it back after the ticked ones", async () => {
    show(section({ enabled: ["claude-code", "fake"], def: "claude-code", offered: ["claude-code", "codex"] }));
    const g = await group();
    expect(g).toHaveTextContent("Also enabled: fake");
    fireEvent.click(within(g).getByLabelText("OpenAI Codex"));
    expect(g).toHaveTextContent("Also enabled: fake");
    fireEvent.click(within(g).getByLabelText("OpenAI Codex"));
    expect(within(g).getByLabelText("OpenAI Codex")).not.toBeChecked();
    expect(g).toHaveTextContent("Also enabled: fake");
  });

  it("lets the last box be unticked in the draft", async () => {
    show(section({ enabled: ["claude-code"], def: "claude-code", offered: ["claude-code"] }));
    const g = await group();
    fireEvent.click(within(g).getByLabelText("Claude Code"));
    expect(within(g).getByLabelText("Claude Code")).not.toBeChecked();
  });

  it("reads a Helm-locked list as names", async () => {
    show(section({ enabled: ["claude-code", "goose"], def: "claude-code", offered: null, lockedEnabled: true }));
    expect(await screen.findByTestId("hosted.enabled_harnesses-locked-value")).toHaveTextContent("Claude Code, goose");
    expect(screen.queryByRole("group", { name: "Programs agents may run" })).toBeNull();
  });
});

describe("Program new agents use", () => {
  it("lists only the ticked programs, by name, in choice order", async () => {
    show(section({ enabled: ["goose", "claude-code"], def: "claude-code", offered: ["claude-code", "codex", "goose"] }));
    await group();
    expect(Array.from(picker().options).map((o) => o.textContent)).toEqual(["Claude Code", "goose"]);
    fireEvent.click(within(await group()).getByLabelText("OpenAI Codex"));
    expect(Array.from(picker().options).map((o) => o.textContent)).toEqual(["Claude Code", "OpenAI Codex", "goose"]);
  });

  it("moves to the first ticked program when a tick removes it, and says so", async () => {
    show(section({ enabled: ["claude-code", "goose"], def: "claude-code", offered: ["claude-code", "codex", "goose"] }));
    const g = await group();
    fireEvent.click(within(g).getByLabelText("Claude Code"));
    await waitFor(() => expect(picker().value).toBe("goose"));
    expect(screen.getByText("Changed to goose because Claude Code is no longer ticked.")).toBeInTheDocument();
  });

  it("leaves a Helm-locked default alone and reads it as a name", async () => {
    show(section({ enabled: ["claude-code", "goose"], def: "goose", offered: null, lockedDefault: true }));
    const g = await group();
    fireEvent.click(within(g).getByLabelText("goose"));
    expect(screen.getByTestId("hosted.default_harness-locked-value")).toHaveTextContent("goose");
    expect(screen.queryByText(/Changed to/)).toBeNull();
  });

  it("shows a default that is not a choice as its own option", async () => {
    show(section({ enabled: ["claude-code", "fake"], def: "fake", offered: null }));
    await group();
    expect(Array.from(picker().options).map((o) => o.textContent)).toEqual(["Claude Code", "fake"]);
    expect(picker().value).toBe("fake");
  });
});

describe("Runners table", () => {
  it("shows display names, and an id that is not a choice raw", async () => {
    show(
      section({
        enabled: ["claude-code"], def: "claude-code", offered: ["claude-code"],
        runners: [{ id: "r1", name: "main", sandbox_memory_mib: 1, sandbox_cpus: 1, sandbox_pids: 1, home_gib: 1, max_sandboxes: 1, harnesses: ["claude-code", "goose", "fake"] }],
      }),
    );
    const row = await screen.findByRole("row", { name: /main/ });
    expect(row).toHaveTextContent("Claude Code, goose, fake");
  });
});

describe("ChoiceListEditor", () => {
  it("keeps values that are not choices, after the chosen ones", () => {
    const onChange = vi.fn();
    const field = { key: "k", label: "L", help: "", kind: "list", choices: CHOICES.slice(0, 2), default_display: "", unit: null, doc: null };
    render(<ChoiceListEditor id="k" field={field as never} value={["codex", "fake"]} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Claude Code"));
    expect(onChange).toHaveBeenCalledWith(["claude-code", "codex", "fake"]);
  });
});
