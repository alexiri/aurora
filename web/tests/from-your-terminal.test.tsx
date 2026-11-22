import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ComputerTab } from "@/components/agents/computer-tab";
import { KeysTab } from "@/components/settings/keys-tab";
import { FromYourTerminal } from "@/components/ui/from-your-terminal";
import { json } from "./support/api";
import { seedMoreControls } from "./support/render";

// P5-REFINE-19 item 6: "From your terminal" is an Expert fold of copyable commands; only commands the CLI has.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "ops", handle: "dani" }),
  useRouter: () => ({ replace: vi.fn(), push: vi.fn() }),
  usePathname: () => "/o/acme/w/ops/agents/dani",
  useSearchParams: () => new URLSearchParams(),
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function mount(ui: React.ReactNode, more: boolean) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seedMoreControls(qc, more);
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

function serve(ssh: boolean) {
  const sandbox = {
    agent_id: "a1", state: "running", boot_id: "b1", runner_id: null, image: null, started_at: "2026-11-20T23:55:00Z", keep_awake_reasons: [],
    failure_code: null, failure_message: null, start_attempt: null, retryable: false, start_log: [], processes: [], ports: [], restart_to_update: false, idle_stop_after_secs: 1200,
    computer: { word: "on", ready: true, working: false, asleep: false, phase: null, sleeps_in_secs: null, started_at: "2026-11-20T23:55:00Z" },
  };
  const routes: Record<string, () => Response> = {
    "GET /v1/agents/a1/sandbox": () => json(200, sandbox),
    "GET /v1/me": () => json(200, { user: { id: "u1", display_name: "Ada", email: "a@b.c", is_instance_admin: false }, memberships: [] }),
    "GET /v1/instance": () => json(200, { version: "0", features: { hosted: true, previews: false, terminal: true, ssh } }),
    "GET /v1/workspaces/w1/api-keys": () => json(200, { items: [], next_cursor: null }),
    // (FIX-218, C-1145) The Terminal card's shell preference.
    "GET /v1/agents/a1/terminal/shell": () => json(200, { shell: "bash", stored: false }),
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : null;
      const url = new URL(req ? req.url : String(input), "http://localhost");
      const h = routes[`${req?.method ?? init?.method ?? "GET"} ${url.pathname}`];
      return h ? h() : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

describe("the fold", () => {
  it("is closed, titled, and lists each command with its explanation", () => {
    mount(<FromYourTerminal commands={[{ label: "the login command", text: "aurora login", note: "Sign in once." }]} />, true);
    const fold = screen.getByTestId("from-your-terminal") as HTMLDetailsElement;
    expect(fold.open).toBe(false);
    expect(within(fold).getByText("From your terminal")).toBeInTheDocument();
    expect(within(fold).getByLabelText("the login command")).toHaveTextContent("aurora login");
    expect(within(fold).getByText("Sign in once.")).toBeInTheDocument();
    expect(within(fold).getByRole("button", { name: "Copy the login command" })).toBeInTheDocument();
  });

  it("does not exist with Show more controls off", () => {
    mount(<FromYourTerminal commands={[{ label: "x", text: "aurora login", note: "n" }]} />, false);
    expect(screen.queryByTestId("from-your-terminal")).toBeNull();
  });
});

describe("where it is", () => {
  it("API keys: aurora login and the curl call, Expert only", async () => {
    serve(true);
    mount(<KeysTab wsId="w1" workspaceName="Ops" />, true);
    const fold = await screen.findByTestId("from-your-terminal");
    expect(within(fold).getByLabelText("the aurora login command")).toHaveTextContent("aurora login");
    expect(within(fold).getByLabelText("the curl command")).toHaveTextContent('curl -H "Authorization: Bearer $AURORA_API_KEY"');
    cleanup();
    mount(<KeysTab wsId="w1" workspaceName="Ops" />, false);
    await screen.findByRole("heading", { name: "API keys" });
    expect(screen.queryByTestId("from-your-terminal")).toBeNull();
  });

  it("Computer: aurora ssh and aurora run with the handle; ssh only where the instance has it", async () => {
    serve(true);
    mount(<ComputerTab agentId="a1" agentName="Dani" agentHandle="dani" terminal />, true);
    const fold = await screen.findByTestId("from-your-terminal");
    expect(within(fold).getByLabelText("the aurora ssh command")).toHaveTextContent("aurora ssh dani");
    expect(within(fold).getByLabelText("the aurora run command")).toHaveTextContent("aurora run --agent dani -- <command>");
    cleanup();
    serve(false);
    mount(<ComputerTab agentId="a1" agentName="Dani" agentHandle="dani" terminal />, true);
    const fold2 = await screen.findByTestId("from-your-terminal");
    expect(within(fold2).queryByLabelText("the aurora ssh command")).toBeNull();
    expect(within(fold2).getByLabelText("the aurora run command")).toBeInTheDocument();
  });

  it("Computer without Show more controls has no fold", async () => {
    serve(true);
    mount(<ComputerTab agentId="a1" agentName="Dani" agentHandle="dani" terminal />, false);
    await screen.findByTestId("computer-facts");
    expect(screen.queryByTestId("from-your-terminal")).toBeNull();
  });
});

describe("the commands exist in the CLI", () => {
  // The fold shows `aurora login`, `aurora ssh <handle>` and `aurora run --agent <handle> -- <command>`: each is a clap
  // subcommand (and `--agent` a flag of `run`) in services/aurora-cli/src/main.rs, the CLI's `--help` source.
  const main = readFileSync(join(__dirname, "../../services/aurora-cli/src/main.rs"), "utf8");
  it("login, run (with --agent) and ssh are subcommands", () => {
    expect(main).toMatch(/\n    Login \{/);
    expect(main).toMatch(/\n    Run \{\s*\n\s*#\[arg\(long\)\]\s*\n\s*agent: String,/);
    expect(main).toMatch(/\n    Ssh\(ssh::SshCmd\),/);
  });
});
