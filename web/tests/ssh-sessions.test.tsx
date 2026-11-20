import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";

import { SshKeysView } from "@/components/account/ssh-keys-view";
import { SshLine } from "@/components/admin/ssh-line";
import { SshPage } from "@/components/agents/ssh-page";
import { ComputerPart } from "@/components/agents/status-rail/parts";
import { wrap } from "./support/render";
import { json } from "./support/api";

// P4-SSH-2 dashboard tests (docs/18 §SSH access, docs/14, C-793, C-795): the open-sessions lists on Account › SSH keys and
// on the agent's SSH page (with the person for someone who can own the agent) and the disconnect confirmation, the Admin ›
// Overview SSH line, and the status rail's link to the SSH page.

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }), usePathname: () => "/account/ssh" }));
vi.mock("next/link", () => ({
  default: ({ href, children, ...rest }: { href: string; children: ReactNode }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));

type Handler = (req: Request, url: URL) => Response | Promise<Response>;
function mockFetch(routes: Record<string, Handler>) {
  const calls: string[] = [];
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
    const url = new URL(req.url);
    calls.push(`${req.method} ${url.pathname}`);
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req, url) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return calls;
}

const instance = (ssh: boolean) => ({
  version: "0",
  features: { hosted: true, channels: false, previews: false, terminal: true, ssh },
});

const AGENT = "11111111-1111-4111-8111-111111111111";
const SID = "22222222-2222-4222-8222-222222222222";

const session = (over: Record<string, unknown> = {}) => ({
  id: SID,
  agent: { id: AGENT, name: "Daneel" },
  workspace_id: "33333333-3333-4333-8333-333333333333",
  key: { id: "44444444-4444-4444-8444-444444444444", name: "work laptop", fingerprint: "SHA256:abc" },
  client_address: "203.0.113.0",
  opened_at: "2026-11-16T10:00:00Z",
  last_activity_at: "2026-11-16T10:05:00Z",
  channels_open: 0,
  channels_total: 3,
  ...over,
});

const facts = {
  host: "ssh.example.org",
  port: 22,
  cert_ttl_secs: 900,
  terminators_live: 1,
  host_keys: [{ public_key: "ssh-ed25519 AAAA", fingerprint: "SHA256:host" }],
  known_hosts: ["ssh.example.org ssh-ed25519 AAAA"],
};

describe("Active SSH sessions on Account › SSH keys", () => {
  const base = {
    "GET /v1/instance": () => json(200, instance(true)),
    "GET /v1/me/ssh-keys": () => json(200, { keys: [] }),
  };

  it("lists the person's sessions with the agent, key, address and channels", async () => {
    mockFetch({ ...base, "GET /v1/me/ssh-sessions": () => json(200, { sessions: [session()] }) });
    wrap(<SshKeysView />);
    const table = await screen.findByRole("table", { name: "Active SSH sessions" });
    expect(within(table).getByText("Daneel")).toBeInTheDocument();
    expect(within(table).getByText("work laptop")).toBeInTheDocument();
    expect(within(table).getByText("203.0.113.0")).toBeInTheDocument();
    expect(within(table).getByText("3")).toBeInTheDocument();
    // A person's own list has no Person column.
    expect(within(table).queryByRole("columnheader", { name: "Person" })).toBeNull();
  });

  it("names the fingerprint once the key was deleted", async () => {
    mockFetch({
      ...base,
      "GET /v1/me/ssh-sessions": () => json(200, { sessions: [session({ key: { id: null, name: null, fingerprint: "SHA256:gone" } })] }),
    });
    wrap(<SshKeysView />);
    expect(await screen.findByText("SHA256:gone (deleted key)")).toBeInTheDocument();
  });

  it("shows the teaching empty state when nothing is open", async () => {
    mockFetch({ ...base, "GET /v1/me/ssh-sessions": () => json(200, { sessions: [] }) });
    wrap(<SshKeysView />);
    await screen.findByText("No active SSH sessions");
    expect(screen.getByText(/appears here while you are connected/)).toBeInTheDocument();
  });

  it("Disconnect asks first, then DELETEs the session and says it ends within 30 seconds", async () => {
    let deleted = 0;
    mockFetch({
      ...base,
      "GET /v1/me/ssh-sessions": () => json(200, { sessions: [session()] }),
      [`DELETE /v1/ssh-sessions/${SID}`]: () => {
        deleted += 1;
        return json(202, { disconnecting: true });
      },
    });
    wrap(<SshKeysView />);
    await screen.findByRole("table", { name: "Active SSH sessions" });
    fireEvent.click(screen.getByRole("button", { name: /Disconnect the session on Daneel/ }));
    const dialog = await screen.findByRole("dialog", { name: "Disconnect SSH session" });
    expect(within(dialog).getByText(/End your SSH session on Daneel/)).toBeInTheDocument();
    expect(deleted).toBe(0);
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(deleted).toBe(0);
    fireEvent.click(screen.getByRole("button", { name: /Disconnect the session on Daneel/ }));
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(deleted).toBe(1));
    expect(await screen.findByText(/The session ends within 30 seconds/)).toBeInTheDocument();
  });

  it("shows the API's refusal when a disconnect fails", async () => {
    mockFetch({
      ...base,
      "GET /v1/me/ssh-sessions": () => json(200, { sessions: [session()] }),
      [`DELETE /v1/ssh-sessions/${SID}`]: () => json(404, { error: { code: "ssh_session_not_found", message: "That session is already closed." } }),
    });
    wrap(<SshKeysView />);
    await screen.findByRole("table", { name: "Active SSH sessions" });
    fireEvent.click(screen.getByRole("button", { name: /Disconnect the session on Daneel/ }));
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Disconnect" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/already closed/);
  });
});

describe("Active sessions on the agent's SSH page", () => {
  const base = {
    "GET /v1/ssh": () => json(200, facts),
    "GET /v1/me/ssh-keys": () => json(200, { keys: [] }),
  };

  it("shows the Person column to someone who can own the agent, and disconnects another person's session", async () => {
    let deleted = 0;
    mockFetch({
      ...base,
      [`GET /v1/agents/${AGENT}/ssh-sessions`]: () =>
        json(200, { sessions: [session({ user: { id: "55555555-5555-4555-8555-555555555555", label: "Bob Builder" } })] }),
      [`DELETE /v1/ssh-sessions/${SID}`]: () => {
        deleted += 1;
        return json(202, { disconnecting: true });
      },
    });
    wrap(<SshPage agentId={AGENT} frozen={false} />);
    const table = await screen.findByRole("table", { name: "Active SSH sessions" });
    expect(within(table).getByRole("columnheader", { name: "Person" })).toBeInTheDocument();
    expect(within(table).getByText("Bob Builder")).toBeInTheDocument();
    // The agent page is about one agent: no Agent column.
    expect(within(table).queryByRole("columnheader", { name: "Agent" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Disconnect the session on Daneel/ }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/End Bob Builder's SSH session on Daneel/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(deleted).toBe(1));
  });

  it("has no Person column in a list of the caller's own sessions and an empty state when none is open", async () => {
    mockFetch({ ...base, [`GET /v1/agents/${AGENT}/ssh-sessions`]: () => json(200, { sessions: [session()] }) });
    wrap(<SshPage agentId={AGENT} frozen={false} />);
    const table = await screen.findByRole("table", { name: "Active SSH sessions" });
    expect(within(table).queryByRole("columnheader", { name: "Person" })).toBeNull();
    cleanup();
    mockFetch({ ...base, [`GET /v1/agents/${AGENT}/ssh-sessions`]: () => json(200, { sessions: [] }) });
    wrap(<SshPage agentId={AGENT} frozen={false} />);
    expect(await screen.findByText(/appears here while someone is connected to this agent/)).toBeInTheDocument();
  });

  it("asks for no sessions while the agent is frozen", async () => {
    const calls = mockFetch({ ...base });
    wrap(<SshPage agentId={AGENT} frozen />);
    await screen.findByText(/This agent is frozen/);
    expect(calls.some((c) => c.includes("ssh-sessions"))).toBe(false);
  });
});

describe("Admin › Overview SSH line", () => {
  const terminator = (over: Record<string, unknown> = {}) => ({
    id: "66666666-6666-4666-8666-666666666666",
    version: "0.1.0",
    host_key_fingerprint: "SHA256:host",
    sessions_open: 2,
    last_seen_at: "2026-11-16T10:00:00Z",
    live: true,
    ...over,
  });

  it("counts the live replicas and their open sessions", async () => {
    mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "GET /v1/admin/terminators": () =>
        json(200, { terminators: [terminator(), terminator({ id: "77777777-7777-4777-8777-777777777777", sessions_open: 1 }), terminator({ id: "88888888-8888-4888-8888-888888888888", live: false, sessions_open: 5 })] }),
    });
    wrap(<SshLine />);
    const line = await screen.findByTestId("ssh-line");
    expect(within(line).getByText("SSH")).toBeInTheDocument();
    expect(within(line).getByText("Terminator up")).toBeInTheDocument();
    expect(within(line).getByText(/2 terminator replicas live, 3 open sessions/)).toBeInTheDocument();
  });

  it("says no terminator was seen when none is live", async () => {
    mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "GET /v1/admin/terminators": () => json(200, { terminators: [terminator({ live: false })] }),
    });
    wrap(<SshLine />);
    const line = await screen.findByTestId("ssh-line");
    expect(within(line).getByText("No terminator seen")).toBeInTheDocument();
    expect(within(line).getByText(/No SSH terminator has reported in the last 90 seconds/)).toBeInTheDocument();
  });

  it("is absent while SSH is off (and asks nothing), and when the list is refused", async () => {
    const calls = mockFetch({ "GET /v1/instance": () => json(200, instance(false)) });
    wrap(<SshLine />);
    await waitFor(() => expect(calls).toContain("GET /v1/instance"));
    expect(screen.queryByTestId("ssh-line")).toBeNull();
    expect(calls.some((c) => c.includes("terminators"))).toBe(false);
    cleanup();
    mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "GET /v1/admin/terminators": () => json(403, { error: { code: "forbidden", message: "no" } }),
    });
    wrap(<SshLine />);
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId("ssh-line")).toBeNull();
  });
});

describe("The status rail's SSH link", () => {
  it("offers SSH beside the other Computer links only when the page exists, and opens it", () => {
    const go = vi.fn();
    const { rerender } = render(<ComputerPart sandbox={undefined} terminalAvailable sshAvailable onNavigate={go} />);
    fireEvent.click(screen.getByRole("button", { name: "SSH" }));
    expect(go).toHaveBeenCalledWith("ssh");
    rerender(<ComputerPart sandbox={undefined} terminalAvailable onNavigate={go} />);
    expect(screen.queryByRole("button", { name: "SSH" })).toBeNull();
    expect(screen.getByRole("button", { name: "Terminal" })).toBeInTheDocument();
  });
});
