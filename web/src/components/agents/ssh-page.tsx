"use client";

// The agent's SSH page (P4-SSH-1, D-239, C-795, docs/14 §Agent page › SSH): how to connect with the Aurora CLI or plain
// ssh, the certificate download, the host key and what works. Hosted agents only, on an instance with `ssh.enabled`; the
// API re-checks `can_use`, BYO and freeze on every request. P4-SSH-2 adds the sessions list (C-793).
import Link from "next/link";
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

/** The certificate as a download named like the file OpenSSH looks for next to `id_ed25519`. */
function CertificateDownload({ cert }: { cert: SshCertificate }) {
  const href = useMemo(
    () => URL.createObjectURL(new Blob([`${cert.certificate}\n`], { type: "text/plain" })),
    [cert.certificate],
  );
  useEffect(() => () => URL.revokeObjectURL(href), [href]);
  return (
    <a
      href={href}
      download="id_ed25519-cert.pub"
      className="inline-flex min-h-10 items-center rounded-control border border-line-strong bg-surface px-3 text-sm font-medium text-ink hover:bg-subtle"
    >
      Download id_ed25519-cert.pub
    </a>
  );
}

const minutes = (secs: number) => (secs % 60 === 0 && secs >= 60 ? `${secs / 60} minute${secs === 60 ? "" : "s"}` : `${secs} seconds`);

export function SshPage({ agentId, frozen }: { agentId: string; frozen: boolean }) {
  // A frozen agent: the header carries the freeze banner; no instructions, no request (the API refuses 423 anyway).
  if (frozen) {
    return (
      <Notice tone="neutral" role="status">
        This agent is frozen. SSH is unavailable while it is frozen.
      </Notice>
    );
  }
  return <SshContent agentId={agentId} />;
}

function SshContent({ agentId }: { agentId: string }) {
  const facts = useSshFacts();
  const keys = useSshKeys();
  const sessions = useAgentSshSessions(agentId);
  const mint = useMintSshCertificate(agentId);
  const [keyId, setKeyId] = useState<string>("");
  const [cert, setCert] = useState<SshCertificate | null>(null);

  if (facts.isPending || keys.isPending) return <Loading />;
  if (facts.isError) return <ErrorState error={facts.error} onRetry={() => facts.refetch()} />;
  if (keys.isError) return <ErrorState error={keys.error} onRetry={() => keys.refetch()} />;

  const f = facts.data;
  const myKeys = keys.data;
  const chosen = myKeys.find((k) => k.id === keyId) ?? myKeys[0];
  const target = `${agentId}@${f.host}`;
  const plain = `ssh -o CertificateFile=~/Downloads/id_ed25519-cert.pub -i ~/.ssh/id_ed25519 -p ${f.port} ${target}`;
  const error = mint.error;
  const rateLimited = error instanceof ApiError && error.code === "ssh_mint_rate_limited";

  return (
    <div className="space-y-6">
      <Section id="ssh-connect" title="Connect with SSH" description="A shell, remote commands, sftp and scp, as the agent's own user." card>
        <div className="space-y-6">
          {/* (P5-REFINE-3) Expert: the CLI snippet. The plain-ssh steps below stay. */}
          <Altitude id="fold.cli" level="expert">
            <div className="space-y-2">
              <h3 className="text-sm font-medium text-ink">Use the Aurora CLI</h3>
              <CopyBlock text={`aurora ssh ${agentId}`} label="the aurora ssh command" />
              <p className="text-xs text-muted">
                Install the Aurora CLI and sign in once with <code className="font-mono">aurora login</code>. It fetches a certificate,
                pins the host key and starts ssh for you.
              </p>
            </div>
          </Altitude>

          <div className="space-y-3">
            <h3 className="text-sm font-medium text-ink">Or use plain ssh</h3>
            <ol className="space-y-4 text-sm text-ink-2">
              <li className="space-y-2">
                <p>
                  <span className="font-medium text-ink">1. Add your key</span> on{" "}
                  <Link href={accountRoutes.ssh} className="text-accent-ink underline">
                    Account › SSH keys
                  </Link>
                  .
                </p>
                {myKeys.length === 0 ? (
                  <Notice tone="pending" role="status">
                    You have no SSH key yet.{" "}
                    <Link href={accountRoutes.ssh} className="font-medium underline">
                      Add one under Account › SSH keys
                    </Link>{" "}
                    to get a certificate.
                  </Notice>
                ) : null}
              </li>
              <li className="space-y-2">
                <p className="font-medium text-ink">2. Get a certificate</p>
                <div className="flex flex-wrap items-end gap-2">
                  {myKeys.length > 1 ? (
                    <div className="space-y-1">
                      <label htmlFor="ssh-cert-key" className="block text-xs text-muted">
                        Key
                      </label>
                      <select
                        id="ssh-cert-key"
                        className={`${inputClass} max-w-full`}
                        value={chosen?.id ?? ""}
                        onChange={(e) => setKeyId(e.target.value)}
                      >
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
                      setCert(null);
                      mint.mutate(chosen.public_key, { onSuccess: setCert });
                    }}
                  >
                    {mint.isPending ? "Getting a certificate…" : "Get a certificate"}
                  </Button>
                  {cert ? <CertificateDownload cert={cert} /> : null}
                </div>
                {cert ? (
                  <p role="status" className="text-xs text-muted">
                    Valid for {minutes(f.cert_ttl_secs)}; get a new one to reconnect later.
                  </p>
                ) : null}
                {error ? (
                  <Notice tone="danger" role="alert">
                    {rateLimited
                      ? `Too many certificates for this agent in the last hour; try again at ${nextHourAt()}.`
                      : (sshErrorText(error) ?? errorMessage(error))}
                  </Notice>
                ) : null}
              </li>
              <li className="space-y-2">
                <p className="font-medium text-ink">3. Connect</p>
                <CopyBlock text={plain} label="the ssh command" />
              </li>
            </ol>
          </div>
        </div>
      </Section>

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

      <Section id="ssh-what-works" title="What works" card>
        <ul className="list-disc space-y-1 pl-5 text-sm text-ink-2">
          <li>An interactive shell, remote commands, sftp and scp.</li>
          <li>
            Port forwarding, agent forwarding and X11 are not available; use the <strong>Previews</strong> page for web apps.
          </li>
          <li>Sessions end after 10 minutes without activity or after 4 hours, and when you lose access to the agent.</li>
        </ul>
      </Section>
    </div>
  );
}
