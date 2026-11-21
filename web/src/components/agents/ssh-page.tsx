"use client";

// The agent's SSH page (P4-SSH-1, D-239, C-795, docs/14 §Agent page › SSH): how to connect with the Aurora CLI or plain
// ssh, the certificate download, the host key and what works. Hosted agents only, on an instance with `ssh.enabled`; the
// API re-checks `can_use`, BYO and freeze on every request. P4-SSH-2 adds the sessions list (C-793).
// (P5-REFINE-15, computer-6) Leads with the Aurora command; the certificate has a countdown, one stable file name, and a
// Download that survives a reload until it expires.
import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useMemo, useState } from "react";

import { CopyBlock } from "@/components/account/ssh-copy";
import { ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { sshKeysHref } from "@/lib/back-link";
import { formatTime } from "@/lib/format-date";
import { accountRoutes } from "@/lib/routes";
import {
  nextHourAt,
  sshErrorText,
  useAgentSshSessions,
  useMintSshCertificate,
  useSshFacts,
  useSshKeys,
  type SshCertificate,
} from "@/lib/api/ssh";
import { SshSessionsList } from "@/components/ssh/sessions-list";

/** The file's name and the path the commands use: stable, so a second download replaces the first (`aurora-daneel-cert.pub`). */
export const certFileName = (handle: string) => `aurora-${handle}-cert.pub`;

const storageKey = (agentId: string, keyId: string) => `aurora.ssh.cert.${agentId}.${keyId}`;

/** A public certificate, not a secret: kept for this tab until it expires so Download survives a reload. Storage errors are ignored. */
function readCert(agentId: string, keyId: string): SshCertificate | null {
  try {
    const raw = window.sessionStorage.getItem(storageKey(agentId, keyId));
    if (!raw) return null;
    const cert = JSON.parse(raw) as SshCertificate;
    if (new Date(cert.valid_before).getTime() > Date.now()) return cert;
    window.sessionStorage.removeItem(storageKey(agentId, keyId));
  } catch {
    // Blocked or corrupt storage: no remembered certificate.
  }
  return null;
}
function writeCert(agentId: string, keyId: string, cert: SshCertificate | null): void {
  try {
    if (cert) window.sessionStorage.setItem(storageKey(agentId, keyId), JSON.stringify(cert));
    else window.sessionStorage.removeItem(storageKey(agentId, keyId));
  } catch {
    // Ignored: the certificate is still on screen for this visit.
  }
}

/** The certificate as a download under its stable name. */
function CertificateDownload({ cert, fileName }: { cert: SshCertificate; fileName: string }) {
  const href = useMemo(
    () => URL.createObjectURL(new Blob([`${cert.certificate}\n`], { type: "text/plain" })),
    [cert.certificate],
  );
  useEffect(() => () => URL.revokeObjectURL(href), [href]);
  return (
    <a
      href={href}
      download={fileName}
      className="inline-flex min-h-10 items-center rounded-control border border-line-strong bg-surface px-3 text-sm font-medium text-ink hover:bg-subtle"
    >
      Download
    </a>
  );
}

/** "Certificate valid for 13 more minutes (until 00:10)", counting down; null once it ran out. */
function validFor(validBefore: string, now: number): string | null {
  const left = new Date(validBefore).getTime() - now;
  if (!(left > 0)) return null;
  const mins = Math.max(1, Math.ceil(left / 60_000));
  return `Certificate valid for ${mins} more ${mins === 1 ? "minute" : "minutes"} (until ${formatTime(validBefore)})`;
}

export function SshPage({ agentId, agentName, agentHandle, frozen }: { agentId: string; agentName?: string; agentHandle?: string; frozen: boolean }) {
  // A frozen agent: the header carries the freeze banner; no instructions, no request (the API refuses 423 anyway).
  if (frozen) {
    return (
      <Notice tone="neutral" role="status">
        This agent is paused. SSH is unavailable while it is paused.
      </Notice>
    );
  }
  return <SshContent agentId={agentId} agentName={agentName} agentHandle={agentHandle} />;
}

function SshContent({ agentId, agentName, agentHandle }: { agentId: string; agentName?: string; agentHandle?: string }) {
  // (P5-REFINE-2, org-3) The Account page offers "← Back to <Agent>": it needs to know where this page is.
  const here = usePathname();
  const keysHref = here.startsWith("/o/") ? sshKeysHref(here) : accountRoutes.ssh;
  const facts = useSshFacts();
  const keys = useSshKeys();
  const sessions = useAgentSshSessions(agentId);
  const mint = useMintSshCertificate(agentId);
  const [keyId, setKeyId] = useState<string>("");
  // Keyed by the key it certifies: each key keeps its own minted certificate until it expires.
  const [minted, setMinted] = useState<Record<string, SshCertificate>>({});
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 15_000);
    return () => clearInterval(t);
  }, []);

  if (facts.isPending || keys.isPending) return <Loading />;
  if (facts.isError) return <ErrorState error={facts.error} onRetry={() => facts.refetch()} />;
  if (keys.isError) return <ErrorState error={keys.error} onRetry={() => keys.refetch()} />;

  const f = facts.data;
  const myKeys = keys.data;
  const chosen = myKeys.find((k) => k.id === keyId) ?? myKeys[0];
  const cert = chosen ? (minted[chosen.id] ?? readCert(agentId, chosen.id)) : null;
  const left = cert ? validFor(cert.valid_before, now) : null;
  const expired = cert !== null && left === null;
  const live = cert !== null && !expired ? cert : null;
  const name = agentName ?? "the agent";
  const handle = agentHandle ?? agentId;
  const file = certFileName(handle);
  // The terminator still takes the agent id as the user name (C-1060): a handle command would not connect, so the id stays
  // behind Copy and the block shows an ellipsis in its place.
  const port = f.port === 22 ? "" : ` -p ${f.port}`;
  const plainFor = (user: string) => `ssh -i ~/.ssh/aurora-${handle} -o CertificateFile=~/.ssh/${file}${port} ${user}@${f.host}`;
  const error = mint.error;
  const rateLimited = error instanceof ApiError && error.code === "ssh_mint_rate_limited";

  return (
    <div className="space-y-6">
      <Section id="ssh-connect" title={`Connect to ${name}'s computer`} description="A shell, remote commands, sftp and scp, as the agent's own user." card>
        <div className="space-y-6">
          <div className="space-y-2">
            <h3 className="text-sm font-medium text-ink">With the Aurora command line:</h3>
            <CopyBlock text={`aurora ssh ${handle}`} label="the aurora ssh command" />
            <Altitude id="fold.cli" level="expert">
              <p className="text-xs text-muted">
                Install the Aurora CLI and sign in once with <code className="font-mono">aurora login</code>. It fetches a certificate,
                pins the host key and starts ssh for you.
              </p>
            </Altitude>
          </div>

          <div className="space-y-3">
            <h3 className="text-sm font-medium text-ink">Plain ssh</h3>
            {myKeys.length === 0 ? (
              <Notice tone="pending" role="status">
                You have no SSH key yet.{" "}
                <Link href={keysHref} className="font-medium underline">
                  Add one under Account › SSH keys
                </Link>{" "}
                to get a certificate.
              </Notice>
            ) : (
              <p className="text-sm text-ink-2">
                Your key is on{" "}
                <Link href={keysHref} className="text-accent-ink underline">
                  Account › SSH keys
                </Link>
                .
              </p>
            )}
            <div className="flex flex-wrap items-end gap-2">
              {myKeys.length > 1 ? (
                <div className="space-y-1">
                  <label htmlFor="ssh-cert-key" className="block text-xs text-muted">
                    Key
                  </label>
                  <select id="ssh-cert-key" className={`${inputClass} max-w-full`} value={chosen?.id ?? ""} onChange={(e) => setKeyId(e.target.value)}>
                    {myKeys.map((k) => (
                      <option key={k.id} value={k.id}>
                        {k.name}
                      </option>
                    ))}
                  </select>
                </div>
              ) : null}
              <Button
                disabled={!chosen || mint.isPending}
                onClick={() => {
                  if (!chosen) return;
                  const id = chosen.id;
                  mint.mutate(chosen.public_key, {
                    onSuccess: (c) => {
                      setMinted((m) => ({ ...m, [id]: c }));
                      writeCert(agentId, id, c);
                    },
                  });
                }}
              >
                {mint.isPending ? "Getting a certificate…" : cert ? "Get a new certificate" : "Get a certificate"}
              </Button>
              {live ? <CertificateDownload cert={live} fileName={file} /> : null}
            </div>
            {cert ? (
              <p role="status" className="text-sm text-ink-2" data-testid="cert-state">
                {left ?? "Expired · Get a new certificate"}
              </p>
            ) : null}
            {live ? (
              <p className="text-xs text-muted">
                Save it as <code className="font-mono">~/.ssh/{file}</code>. Download stays until the certificate expires, and always saves under the same
                name; replace the old file if your browser adds a number.
              </p>
            ) : null}
            {error ? (
              <Notice tone="danger" role="alert">
                {rateLimited
                  ? `Too many certificates for this agent in the last hour; try again at ${nextHourAt()}.`
                  : (sshErrorText(error) ?? errorMessage(error))}
              </Notice>
            ) : null}
            <CopyBlock text={plainFor(agentId)} display={plainFor("…")} label="the ssh command" />
          </div>
        </div>
      </Section>

      <Altitude id="ssh.sessions" level="expert">
        <Section id="ssh-host-key" title="Host key" card>
          {f.terminators_live === 0 ? (
            <Notice tone="pending" role="status">
              SSH is enabled but no SSH server is running; ask your administrator.
            </Notice>
          ) : (
            <div className="space-y-3">
              <CopyBlock text={f.known_hosts.join("\n")} label="the known_hosts lines" />
              <ul className="space-y-1">
                {f.host_keys.map((k) => (
                  <li key={k.fingerprint} className="break-all font-mono text-xs text-ink-2">
                    {k.fingerprint}
                  </li>
                ))}
              </ul>
              <p className="text-xs text-muted">
                Check that ssh shows this fingerprint the first time you connect, or add the line above to{" "}
                <code className="font-mono">~/.ssh/known_hosts</code>.
              </p>
            </div>
          )}
        </Section>

        <Section
          id="ssh-sessions"
          title="Active sessions"
          description="Open SSH connections to this agent. Someone who can manage the agent sees everyone's; otherwise only yours."
        >
          <SshSessionsList query={sessions} scope="agent" emptyWhen="A session appears here while someone is connected to this agent with ssh, sftp or scp." />
        </Section>
      </Altitude>

      <Section id="ssh-what-works" title="What works" card>
        <ul className="list-disc space-y-1 pl-5 text-sm text-ink-2">
          <li>An interactive shell, remote commands, sftp and scp.</li>
          <li>
            Port forwarding, agent forwarding and X11 are not available; use the <strong>Previews</strong> page for web apps.
          </li>
          <li>A session with no output stays open while your SSH client is connected, up to 4 hours.</li>
          <li>A session also ends when you lose access to the agent, or after 10 minutes in which your client sends nothing at all.</li>
        </ul>
      </Section>
    </div>
  );
}
