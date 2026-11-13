"use client";

// Port previews (P4-PRV-1, D-144, docs/14): the agent's shares. The tab exists only for hosted
// agents when the instance runs previews; `orgOn` says the org switch is on (members without it
// see nothing here but the note). Sharing a port is a person's act: agents cannot reach these
// routes (SEC-80).
import { useMemo, useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { useSandbox } from "@/lib/api/conversations";
import {
  useCreatePreview,
  useDeletePreview,
  usePatchPreview,
  usePreviews,
  useStartPreview,
  type PreviewShare,
} from "@/lib/api/previews";
import { fmtDate } from "@/components/account/key-table";

const STATUS_TONE: Record<string, "accent" | "pending" | "neutral" | "danger"> = {
  listening: "accent",
  waking: "pending",
  not_listening: "neutral",
  paused: "neutral",
  frozen: "neutral",
};
const STATUS_WORD: Record<string, string> = {
  listening: "Listening",
  waking: "Waking",
  not_listening: "Not listening",
  paused: "Paused",
  frozen: "Frozen",
};

/** The status word is part of the chip text, never colour alone. */
function StatusChip({ status }: { status: string }) {
  return (
    <Badge tone={STATUS_TONE[status] ?? "neutral"} className="normal-case tracking-normal">
      {STATUS_WORD[status] ?? status}
    </Badge>
  );
}

export function ShareDialog({
  agentId,
  initial,
  prefillPort,
  onDone,
}: {
  agentId: string;
  initial?: PreviewShare;
  /** From the Computer tab's Share… action: the port is fixed and known to be listening. */
  prefillPort?: number;
  onDone: () => void;
}) {
  const create = useCreatePreview(agentId);
  const patch = usePatchPreview(agentId);
  const sandbox = useSandbox(agentId);
  const editing = initial !== undefined;
  const [port, setPort] = useState(initial ? String(initial.port) : prefillPort ? String(prefillPort) : "");
  const [command, setCommand] = useState(initial?.start_command ?? "");
  const [cwd, setCwd] = useState(initial?.cwd ?? "");
  const [portInvalid, setPortInvalid] = useState(false);
  const mut = editing ? patch : create;
  const fe = mut.error ? fieldErrors(mut.error) : {};
  const listening = useMemo(
    () => (sandbox.data?.ports ?? []).map((p) => p.port).sort((a, b) => a - b),
    [sandbox.data],
  );

  function submit(e: FormEvent) {
    e.preventDefault();
    if (editing) {
      patch.mutate(
        {
          id: initial.id,
          body: {
            start_command: command.trim() === "" ? null : command.trim(),
            cwd: cwd.trim() === "" ? null : cwd.trim(),
          },
        },
        { onSuccess: onDone },
      );
    } else {
      // The same bounds the server enforces; Number("") would otherwise submit 0.
      const n = Number(port);
      if (!(Number.isInteger(n) && n >= 1024 && n <= 65535)) {
        setPortInvalid(true);
        return;
      }
      setPortInvalid(false);
      create.mutate(
        {
          port: n,
          ...(command.trim() === "" ? {} : { start_command: command.trim() }),
          ...(cwd.trim() === "" ? {} : { cwd: cwd.trim() }),
        },
        { onSuccess: onDone },
      );
    }
  }

  return (
    <FormDialog
      open
      onOpenChange={(o) => {
        if (!o) onDone();
      }}
      title={editing ? `Edit share for port ${initial.port}` : "Share a port"}
      description="Private: signed-in people who can use this workspace can open the app at its own aurora address."
      submitLabel={editing ? "Save" : "Share port"}
      busy={mut.isPending}
      onSubmit={submit}
    >
      {!editing ? (
        <>
          <Field
            id="pv-port"
            label="Port"
            error={fe.port ?? (portInvalid ? "Enter a TCP port between 1024 and 65535." : undefined)}
            hint="TCP port inside the agent's computer, 1024–65535."
          >
            <input
              id="pv-port"
              inputMode="numeric"
              className={inputClass}
              value={prefillPort ? String(prefillPort) : port}
              readOnly={prefillPort !== undefined}
              aria-invalid={portInvalid || undefined}
              onChange={(e) => {
                setPort(e.target.value);
                setPortInvalid(false);
              }}
            />
          </Field>
          {prefillPort === undefined ? (
            <div className="flex flex-wrap items-center gap-1.5">
              {listening.length ? (
                listening.map((p) => (
                  <Button key={p} type="button" size="sm" variant="outline" onClick={() => setPort(String(p))}>
                    {p}
                  </Button>
                ))
              ) : (
                <p className="text-xs text-muted">Nothing listening yet — you can still add a start command.</p>
              )}
            </div>
          ) : null}
        </>
      ) : null}
      <Field
        id="pv-command"
        label="Start command (optional)"
        error={fe.start_command}
        hint="Runs inside the agent's computer when someone opens the link and nothing is listening. It can do whatever the agent can do."
      >
        <input
          id="pv-command"
          className={inputClass}
          value={command}
          maxLength={1024}
          onChange={(e) => setCommand(e.target.value)}
        />
      </Field>
      <Field id="pv-cwd" label="Working folder (optional)" error={fe.cwd} hint="Relative to /workspace.">
        <input id="pv-cwd" className={inputClass} value={cwd} maxLength={200} onChange={(e) => setCwd(e.target.value)} />
      </Field>
      {mut.error && Object.keys(fe).length === 0 ? (
        <Notice tone="danger" role="alert">
          {errorMessage(mut.error)}
        </Notice>
      ) : null}
    </FormDialog>
  );
}

function ShareRow({ agentId, s, frozen }: { agentId: string; s: PreviewShare; frozen: boolean }) {
  const del = useDeletePreview(agentId);
  const start = useStartPreview(agentId);
  const upd = usePatchPreview(agentId);
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [lastStart, setLastStart] = useState(false);
  const err = del.error ?? start.error ?? upd.error;
  return (
    <tr>
      <Td className="max-w-64">
        <span className="block break-all font-mono text-xs text-ink-2">{s.hostname}</span>
        <span className="mt-1 flex gap-2 text-xs text-muted">
          <button
            type="button"
            className="text-link underline"
            onClick={() => void navigator.clipboard.writeText(s.url)}
          >
            Copy
          </button>
          <a href={s.url} target="_blank" rel="noopener noreferrer" className="text-link underline">
            Open
          </a>
        </span>
      </Td>
      <Td className="font-mono">{s.port}</Td>
      <Td>
        <StatusChip status={s.status} />
      </Td>
      <Td className="max-w-56">
        {s.start_command ? (
          <button type="button" className="break-all text-left text-link underline" onClick={() => setLastStart(true)}>
            {s.start_command.length > 40 ? `${s.start_command.slice(0, 40)}…` : s.start_command}
          </button>
        ) : (
          <span className="text-muted">—</span>
        )}
      </Td>
      <Td className="whitespace-nowrap text-ink-2">{fmtDate(s.last_start?.at ?? null)}</Td>
      <Td className="whitespace-nowrap text-ink-2">{fmtDate(s.last_accessed_at ?? null)}</Td>
      <Td className="text-right">
        <div className="flex flex-wrap justify-end gap-2">
          {!frozen && s.state === "active" && s.start_command ? (
            <Button size="sm" variant="outline" disabled={start.isPending} onClick={() => start.mutate(s.id)}>
              Start now
            </Button>
          ) : null}
          {!frozen ? (
            <Button
              size="sm"
              variant="outline"
              disabled={upd.isPending}
              onClick={() => upd.mutate({ id: s.id, body: { state: s.state === "active" ? "paused" : "active" } })}
            >
              {s.state === "active" ? "Pause" : "Resume"}
            </Button>
          ) : null}
          {!frozen ? (
            <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
              Edit
            </Button>
          ) : null}
          {!frozen ? (
            <Button size="sm" variant="outline" onClick={() => setConfirming(true)}>
              Delete
            </Button>
          ) : null}
        </div>
      </Td>
      {editing ? <ShareDialog agentId={agentId} initial={s} onDone={() => setEditing(false)} /> : null}
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title={`Delete the share for port ${s.port}?`}
        description="Its address stops working and is never issued again. Anyone with the link gets an error."
        confirmLabel="Delete"
        danger
        busy={del.isPending}
        onConfirm={() => del.mutate(s.id, { onSettled: () => setConfirming(false) })}
      />
      <FormDialog
        open={lastStart}
        onOpenChange={setLastStart}
        title={`Port ${s.port} — last start`}
        submitLabel="Close"
        onSubmit={(e) => {
          e.preventDefault();
          setLastStart(false);
        }}
      >
        <dl className="grid grid-cols-[auto_1fr] items-baseline gap-x-4 gap-y-1 text-sm">
          <dt className="text-muted">Command</dt>
          <dd className="break-all font-mono text-xs">{s.start_command}</dd>
          {s.cwd ? (
            <>
              <dt className="text-muted">Working folder</dt>
              <dd className="break-all font-mono text-xs">{s.cwd}</dd>
            </>
          ) : null}
          {s.last_start ? (
            <>
              <dt className="text-muted">Started</dt>
              <dd>{fmtDate(s.last_start.at)}</dd>
              <dt className="text-muted">Exit code</dt>
              <dd>{s.last_start.exit_code ?? "—"}</dd>
              {s.last_start.tail ? (
                <>
                  <dt className="text-muted">Output tail</dt>
                  <dd className="break-all font-mono text-xs whitespace-pre-wrap">{s.last_start.tail}</dd>
                </>
              ) : null}
            </>
          ) : (
          <dd>Not started yet.</dd>
          )}
        </dl>
      </FormDialog>
      {err ? (
        <Td className="border-t border-line">
          <Notice tone="danger" role="alert">
            {errorMessage(err)}
          </Notice>
        </Td>
      ) : null}
    </tr>
  );
}

export function PreviewsTab({ agentId, frozen }: { agentId: string; frozen: boolean }) {
  const q = usePreviews(agentId);
  const [adding, setAdding] = useState(false);
  if (q.isPending) return <Loading />;
  // 404 previews_disabled: the tab only renders when the instance runs previews, so this is a
  // stale instance flag; the empty message covers it.
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  const shares = q.data.shares;
  return (
    <Section
      id="previews"
      title="Previews"
      description="Addresses people can open to reach a web app this agent runs inside its computer."
      actions={
        !adding && !frozen ? (
          <Button size="sm" onClick={() => setAdding(true)}>
            Add share
          </Button>
        ) : null
      }
    >
      {frozen ? (
        <Notice tone="pending" role="status">
          This agent is frozen: its shares are kept but paused, and nothing can be woken or started.
        </Notice>
      ) : null}
      {adding ? <ShareDialog agentId={agentId} onDone={() => setAdding(false)} /> : null}
      {shares.length === 0 ? (
        <EmptyState title="No shares yet">
          Share a port of the agent&apos;s computer so people can open the app it runs there.
        </EmptyState>
      ) : (
        <DataTable caption="Preview shares">
          <thead>
            <tr>
              <Th>Address</Th>
              <Th>Port</Th>
              <Th>Status</Th>
              <Th>Start command</Th>
              <Th>Last start</Th>
              <Th>Last accessed</Th>
              <Th>
                <span className="sr-only">Actions</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {shares.map((s) => (
              <ShareRow key={s.id} agentId={agentId} s={s} frozen={frozen} />
            ))}
          </tbody>
        </DataTable>
      )}
    </Section>
  );
}
