import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AccountNavList } from "@/components/app-shell";
import { checkSshKey, ONLY_ED25519, PRIVATE_KEY } from "@/lib/ssh-key";
import { accountRoutes } from "@/lib/routes";
import { SshKeysView } from "@/components/account/ssh-keys-view";
import { SshPage } from "@/components/agents/ssh-page";
import { availableSections, isPageAvailable, resolvePage } from "@/components/agents/sections";
import { wrap } from "./support/render";
import { json } from "./support/api";

// P4-SSH-1 dashboard tests (docs/18 §SSH access, docs/14 §Account and §Agent page): the keys view (list, add with inline
// errors, rename, delete confirm, hidden without features.ssh) and the agent's SSH page (certificate download, the empty
// host-key notice, frozen and BYO, the 429 text).

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
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
    const url = new URL(req.url);
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req, url) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

const instance = (ssh: boolean) => ({
  version: "0",
  features: { hosted: true, channels: false, previews: false, terminal: false, ssh },
});

const key = (n: number, extra: Record<string, unknown> = {}) => ({
  id: `00000000-0000-4000-8000-00000000000${n}`,
  name: `laptop ${n}`,
  algorithm: "ssh-ed25519",
  public_key: `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI${n}`,
  fingerprint: `SHA256:gOAeoDRLhPIlNGAongiqcd5fskHrxCKhw3I7so57P3${n}`,
  created_at: "2026-01-01T00:00:00Z",
  last_used_at: null,
  ...extra,
});

const AGENT = "11111111-1111-4111-8111-111111111111";

const facts = (over: Record<string, unknown> = {}) => ({
  host: "ssh.example.org",
  port: 2222,
  cert_ttl_secs: 900,
  terminators_live: 0,
  host_keys: [],
  known_hosts: [],
  ...over,
});

beforeEach(() => {
  vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: vi.fn(() => "blob:cert"), revokeObjectURL: vi.fn() }));
});

describe("SshKeysView", () => {
  it("is hidden (and asks nothing about keys) while the instance has no SSH", async () => {
    const fn = mockFetch({ "GET /v1/instance": () => json(200, instance(false)) });
    wrap(<SshKeysView />);
    await screen.findByText(/SSH access is not enabled/);
    expect(fn.mock.calls.some(([r]) => String((r as Request).url).includes("ssh-keys"))).toBe(false);
  });

  it("lists keys with a truncated fingerprint whose full value is in the tooltip", async () => {
    mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "GET /v1/me/ssh-keys": () => json(200, { keys: [key(1), key(2, { last_used_at: "2026-02-01T00:00:00Z" })] }),
    });
    wrap(<SshKeysView />);
    await screen.findByText("laptop 1");
    const fp = key(1).fingerprint;
    const short = screen.getAllByTitle(fp)[0]!;
    expect(short.textContent).toContain("…");
    expect(short.textContent).not.toBe(fp);
    expect(screen.getByRole("button", { name: "Copy fingerprint of laptop 1" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Rename laptop 2" })).toBeInTheDocument();
  });

  it("shows the teaching empty state with the Add key action", async () => {
    mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "GET /v1/me/ssh-keys": () => json(200, { keys: [] }),
    });
    wrap(<SshKeysView />);
    await screen.findByText("No SSH keys yet");
    expect(screen.getByRole("button", { name: "Add key" })).toBeInTheDocument();
  });

  it("adds a key and maps each refusal to plain text under the field", async () => {
    let post = 0;
    const answers = [
      json(422, { error: { code: "ssh_key_invalid", message: "x" } }),
      json(409, { error: { code: "ssh_key_exists", message: "x" } }),
      json(409, { error: { code: "ssh_key_limit", message: "x" } }),
      json(201, key(3)),
    ];
    const bodies: unknown[] = [];
    mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "GET /v1/me/ssh-keys": () => json(200, { keys: [] }),
      "POST /v1/me/ssh-keys": async (req) => {
        bodies.push(await req.json());
        return answers[post++]!;
      },
    });
    wrap(<SshKeysView />);
    await screen.findByText("No SSH keys yet");
    fireEvent.click(screen.getByRole("button", { name: "Add key" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/Only ed25519 keys are accepted; the private key never leaves your computer/)).toBeInTheDocument();
    expect(within(dialog).getByText("ssh-keygen -t ed25519")).toBeInTheDocument();
    const submit = within(dialog).getByRole("button", { name: "Add key" });
    expect(submit).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText("Public key"), { target: { value: ` ${GOOD} ` } });
    fireEvent.change(within(dialog).getByLabelText(/Name/), { target: { value: "Work" } });
    for (const text of [/not an ed25519 public key/, /already added this key/, /at most 10 keys/]) {
      fireEvent.click(within(dialog).getByRole("button", { name: "Add key" }));
      expect(await within(dialog).findByText(text)).toBeInTheDocument();
    }
    expect(bodies[0]).toEqual({ public_key: GOOD, name: "Work" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add key" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(post).toBe(4);
  });

  it("renames a key", async () => {
    const patches: unknown[] = [];
    mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "GET /v1/me/ssh-keys": () => json(200, { keys: [key(1)] }),
      [`PATCH /v1/me/ssh-keys/${key(1).id}`]: async (req) => {
        patches.push(await req.json());
        return json(200, key(1, { name: "Desk" }));
      },
    });
    wrap(<SshKeysView />);
    fireEvent.click(await screen.findByRole("button", { name: "Rename laptop 1" }));
    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText("Name") as HTMLInputElement;
    expect(input.value).toBe("laptop 1");
    fireEvent.change(input, { target: { value: " Desk " } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(patches).toEqual([{ name: "Desk" }]));
  });

  it("asks before deleting and says open sessions end within 30 seconds", async () => {
    let deleted = 0;
    mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "GET /v1/me/ssh-keys": () => json(200, { keys: [key(1)] }),
      [`DELETE /v1/me/ssh-keys/${key(1).id}`]: () => {
        deleted += 1;
        return new Response(null, { status: 204 });
      },
    });
    wrap(<SshKeysView />);
    fireEvent.click(await screen.findByRole("button", { name: "Delete laptop 1" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/end within 30 seconds/)).toBeInTheDocument();
    expect(deleted).toBe(0);
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(deleted).toBe(0);
    fireEvent.click(screen.getByRole("button", { name: "Delete laptop 1" }));
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Delete key" }));
    await waitFor(() => expect(deleted).toBe(1));
  });
});

describe("SshPage", () => {
  const base = (over: Record<string, Handler> = {}) =>
    mockFetch({
      "GET /v1/ssh": () => json(200, facts()),
      "GET /v1/me/ssh-keys": () => json(200, { keys: [key(1)] }),
      ...over,
    });

  it("leads with the Aurora command (the handle, not the id) and shows the plain ssh command with the id in full", async () => {
    base();
    // The host key and the sessions are Expert (D-406, `ssh.sessions`).
    wrap(<SshPage agentId={AGENT} agentName="Daneel" agentHandle="daneel" frozen={false} />, { moreControls: true });
    await screen.findByText(/no SSH server is running/);
    expect(screen.getByRole("heading", { name: "Connect to Daneel's computer" })).toBeInTheDocument();
    expect(screen.getByText("With the Aurora command line:")).toBeInTheDocument();
    expect(screen.getByLabelText("the aurora ssh command").textContent).toBe("aurora ssh daneel");
    // The terminator still wants the agent id as the user name: the block shows it, exactly what Copy puts on the clipboard.
    expect(screen.getByLabelText("the ssh command").textContent).toBe(
      `ssh -i ~/.ssh/aurora-daneel -o CertificateFile=~/.ssh/aurora-daneel-cert.pub -p 2222 ${AGENT}@ssh.example.org`,
    );
    expect(screen.getByRole("button", { name: "Copy the ssh command" })).toBeInTheDocument();
    expect(screen.getByText(/Port forwarding, agent forwarding and X11 are not available/)).toBeInTheDocument();
    // D-434, FIX-199: a quiet session stays open while the client is connected (its keepalives count).
    expect(
      screen.getByText("A session with no output stays open while your SSH client is connected, up to 4 hours."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("A session also ends when you lose access to the agent, or after 10 minutes in which your client sends nothing at all."),
    ).toBeInTheDocument();
  });

  it("copies the full command, with the id, from Copy", async () => {
    base();
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    wrap(<SshPage agentId={AGENT} agentName="Daneel" agentHandle="daneel" frozen={false} />);
    fireEvent.click(await screen.findByRole("button", { name: "Copy the ssh command" }));
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(
        `ssh -i ~/.ssh/aurora-daneel -o CertificateFile=~/.ssh/aurora-daneel-cert.pub -p 2222 ${AGENT}@ssh.example.org`,
      ),
    );
  });

  it("keeps the host key and the sessions behind the Expert switch", async () => {
    base();
    wrap(<SshPage agentId={AGENT} agentName="Daneel" agentHandle="daneel" frozen={false} />, { moreControls: false });
    await screen.findByText("With the Aurora command line:");
    expect(screen.queryByText(/no SSH server is running/)).not.toBeInTheDocument();
    expect(screen.queryByText("Active sessions")).not.toBeInTheDocument();
  });

  it("shows the known_hosts lines and fingerprints once a terminator registered", async () => {
    base({
      "GET /v1/ssh": () =>
        json(
          200,
          facts({
            terminators_live: 2,
            host_keys: [{ public_key: "ssh-ed25519 AAAAHOST", fingerprint: "SHA256:hostfp" }],
            known_hosts: ["[ssh.example.org]:2222 ssh-ed25519 AAAAHOST"],
          }),
        ),
    });
    wrap(<SshPage agentId={AGENT} frozen={false} />, { moreControls: true });
    await screen.findByText("SHA256:hostfp");
    expect(screen.getByLabelText("the known_hosts lines").textContent).toBe("[ssh.example.org]:2222 ssh-ed25519 AAAAHOST");
    expect(screen.queryByText(/no SSH server is running/)).not.toBeInTheDocument();
  });

  const certBody = (validBefore: string) => ({
    certificate: "ssh-ed25519-cert-v01@openssh.com AAAACERT aurora:agent",
    serial: "7",
    principal: AGENT,
    key_id: "k",
    valid_after: "2026-01-01T00:00:00Z",
    valid_before: validBefore,
    username: AGENT,
    host: "ssh.example.org",
    port: 2222,
    known_hosts: [],
  });

  it("mints a certificate for the registered key, offers it under one stable name and counts down", async () => {
    const bodies: unknown[] = [];
    const until = new Date(Date.now() + 13 * 60_000 + 30_000).toISOString();
    base({
      [`POST /v1/agents/${AGENT}/ssh-certificate`]: async (req) => {
        bodies.push(await req.json());
        return json(201, certBody(until));
      },
    });
    wrap(<SshPage agentId={AGENT} agentName="Daneel" agentHandle="daneel" frozen={false} />);
    fireEvent.click(await screen.findByRole("button", { name: "Get a certificate" }));
    const link = await screen.findByRole("link", { name: "Download" });
    expect(link).toHaveAttribute("download", "aurora-daneel-cert.pub");
    expect(screen.getByTestId("cert-state").textContent).toMatch(/^Certificate valid for 14 more minutes \(until \d{1,2}:\d{2}.*\)$/);
    expect(screen.getByText(/Save it as/).textContent).toContain("~/.ssh/aurora-daneel-cert.pub");
    expect(screen.getByRole("button", { name: "Get a new certificate" })).toBeInTheDocument();
    expect(bodies).toEqual([{ public_key: key(1).public_key }]);
  });

  it("keeps Download after a reload until the certificate expires (sessionStorage), and says Expired afterwards", async () => {
    window.sessionStorage.clear();
    const until = new Date(Date.now() + 5 * 60_000).toISOString();
    window.sessionStorage.setItem(`aurora.ssh.cert.${AGENT}.${key(1).id}`, JSON.stringify(certBody(until)));
    base();
    const first = wrap(<SshPage agentId={AGENT} agentHandle="daneel" frozen={false} />);
    expect(await screen.findByRole("link", { name: "Download" })).toHaveAttribute("download", "aurora-daneel-cert.pub");
    expect(screen.getByTestId("cert-state").textContent).toContain("Certificate valid for 5 more minutes");
    first.unmount();
    window.sessionStorage.setItem(`aurora.ssh.cert.${AGENT}.${key(1).id}`, JSON.stringify(certBody(new Date(Date.now() - 1000).toISOString())));
    wrap(<SshPage agentId={AGENT} agentHandle="daneel" frozen={false} />);
    await screen.findByRole("button", { name: "Get a certificate" });
    expect(screen.queryByRole("link", { name: "Download" })).not.toBeInTheDocument();
    expect(window.sessionStorage.getItem(`aurora.ssh.cert.${AGENT}.${key(1).id}`)).toBeNull();
  });

  it("lets a person with several keys pick one", async () => {
    const bodies: { public_key: string }[] = [];
    base({
      "GET /v1/me/ssh-keys": () => json(200, { keys: [key(1), key(2)] }),
      [`POST /v1/agents/${AGENT}/ssh-certificate`]: async (req) => {
        bodies.push((await req.json()) as { public_key: string });
        return json(422, { error: { code: "ssh_key_not_registered", message: "x" } });
      },
    });
    wrap(<SshPage agentId={AGENT} frozen={false} />);
    const select = await screen.findByLabelText("Key");
    fireEvent.change(select, { target: { value: key(2).id } });
    fireEvent.click(screen.getByRole("button", { name: "Get a certificate" }));
    await screen.findByText(/not registered/);
    expect(bodies[0]!.public_key).toBe(key(2).public_key);
  });

  it("says what to do without a key, and disables the certificate button", async () => {
    base({ "GET /v1/me/ssh-keys": () => json(200, { keys: [] }) });
    wrap(<SshPage agentId={AGENT} frozen={false} />);
    await screen.findByText(/You have no SSH key yet/);
    expect(screen.getByRole("link", { name: /Add one under Account › SSH keys/ })).toHaveAttribute("href", "/account/ssh");
    expect(screen.getByRole("button", { name: "Get a certificate" })).toBeDisabled();
  });

  it("reads a 429 as 'try again at HH:MM'", async () => {
    base({
      [`POST /v1/agents/${AGENT}/ssh-certificate`]: () =>
        json(429, { error: { code: "ssh_mint_rate_limited", message: "x" } }),
    });
    wrap(<SshPage agentId={AGENT} frozen={false} />);
    fireEvent.click(await screen.findByRole("button", { name: "Get a certificate" }));
    await screen.findByText(/Too many certificates for this agent in the last hour; try again at \d{1,2}:\d{2}/);
  });

  it("shows only a notice for a frozen agent and asks the API nothing", async () => {
    const fn = base();
    wrap(<SshPage agentId={AGENT} frozen={true} />);
    await screen.findByText(/paused/);
    expect(screen.queryByRole("button", { name: "Get a certificate" })).not.toBeInTheDocument();
    expect(fn).not.toHaveBeenCalled();
  });
});

const GOOD = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN5ElGab515YLw7PcnKusC/LWNtScxqkSkx78EjhVfhl bob@x";

describe("Account's rail lists SSH keys only while features.ssh is on (account-4)", () => {
  it("has SSH keys between Notifications and API keys while features.ssh is on", async () => {
    mockFetch({ "GET /v1/instance": () => json(200, instance(true)) });
    wrap(<AccountNavList pathname="/account" />);
    const link = await screen.findByRole("link", { name: "SSH keys" });
    expect(link).toHaveAttribute("href", accountRoutes.ssh);
    expect(accountRoutes.ssh).toBe("/account/ssh");
    expect(screen.getAllByRole("link").map((l) => l.textContent)).toEqual(["Profile", "Security", "Notifications", "SSH keys", "API keys", "Chat platforms", "Delete account"]);
  });

  it("has no SSH keys entry with features.ssh off", async () => {
    mockFetch({ "GET /v1/instance": () => json(200, instance(false)) });
    wrap(<AccountNavList pathname="/account" />);
    await screen.findByRole("link", { name: "Notifications" });
    expect(screen.queryByRole("link", { name: "SSH keys" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("link").map((l) => l.textContent)).toEqual(["Profile", "Security", "Notifications", "API keys", "Chat platforms", "Delete account"]);
  });
});

describe("the SSH key as it is pasted (account-5)", () => {
  it("accepts one ed25519 line and refuses everything else with the right words", () => {
    expect(checkSshKey("")).toEqual({ state: "empty" });
    expect(checkSshKey(GOOD)).toEqual({ state: "ok" });
    expect(checkSshKey("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC7abcdefghijklmnop")).toEqual({ state: "unsupported", message: ONLY_ED25519 });
    expect(checkSshKey("ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTY")).toMatchObject({ state: "unsupported" });
    expect(checkSshKey(`${GOOD}\n${GOOD}`)).toMatchObject({ state: "unsupported" });
    expect(checkSshKey("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----")).toEqual({ state: "private", message: PRIVATE_KEY });
  });

  it("an RSA paste says so at once and Add key stays disabled; a private key is refused and never sent", async () => {
    const bodies: unknown[] = [];
    mockFetch({
      "GET /v1/instance": () => json(200, instance(true)),
      "GET /v1/me/ssh-keys": () => json(200, { keys: [] }),
      "POST /v1/me/ssh-keys": async (req) => {
        bodies.push(await req.json());
        return json(201, key(1));
      },
    });
    wrap(<SshKeysView />);
    await screen.findByText("No SSH keys yet");
    fireEvent.click(screen.getByRole("button", { name: "Add key" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Public key"), { target: { value: "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC7abcdefghijklmnop" } });
    expect(within(dialog).getByText(ONLY_ED25519)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Add key" })).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText("Public key"), { target: { value: "-----BEGIN OPENSSH PRIVATE KEY-----\nabc" } });
    expect(within(dialog).getByText(PRIVATE_KEY)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Add key" })).toBeDisabled();
    expect(bodies).toEqual([]);
  });
});

describe("the SSH page's availability", () => {
  const on = { byo: false, previews: false, terminal: false, ssh: true };
  it("exists for hosted agents only while features.ssh is on, independent of the terminal", () => {
    expect(isPageAvailable("ssh", on)).toBe(true);
    expect(isPageAvailable("ssh", { ...on, ssh: false })).toBe(false);
    expect(isPageAvailable("ssh", { byo: false, previews: false, terminal: false })).toBe(false);
    expect(isPageAvailable("ssh", { ...on, byo: true })).toBe(false);
    expect(resolvePage("ssh", { ...on, ssh: false })).toBe("chat");
    const computer = availableSections({ ...on, terminal: true, previews: true }).find((s) => s.id === "computer")!;
    expect(computer.pages.map((p) => p.label)).toEqual(["Files", "Terminal", "Connect", "Previews", "Details"]);
  });
});
