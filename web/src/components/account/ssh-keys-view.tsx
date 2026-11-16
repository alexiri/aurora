"use client";

// Account › SSH keys (P4-SSH-1, D-239, C-790, C-795, docs/14 §Account): the person's registered ed25519 public keys, with
// Add key, Rename and Delete. Writes need the dashboard session; the page exists only while `features.ssh` is on.
import { useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { useInstance } from "@/lib/api/queries";
import { errorMessage } from "@/lib/api/errors";
import { sshErrorText, useAddSshKey, useDeleteSshKey, useRenameSshKey, useSshKeys, type SshKey } from "@/lib/api/ssh";

import { CopyBlock, CopyButton, middleTruncate } from "./ssh-copy";

const fmtDate = (s?: string | null) => (s ? new Date(s).toLocaleString() : "—");

function AddKeyDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const add = useAddSshKey();
  const [name, setName] = useState("");
  const [line, setLine] = useState("");
  const reset = () => {
    setName("");
    setLine("");
    add.reset();
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    add.mutate(
      { public_key: line.trim(), name: name.trim() || undefined },
      {
        onSuccess: () => {
          reset();
          onOpenChange(false);
        },
      },
    );
  };
  const text = add.error ? (sshErrorText(add.error) ?? errorMessage(add.error)) : undefined;
  return (
    <FormDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) reset();
        onOpenChange(o);
      }}
      title="Add SSH key"
      description="Paste the contents of ~/.ssh/id_ed25519.pub. Only ed25519 keys are accepted; the private key never leaves your computer."
      submitLabel="Add key"
      busy={add.isPending}
      submitDisabled={line.trim() === ""}
      onSubmit={submit}
    >
      <Field id="ssh-key-name" label="Name (optional)" hint="Shown in this list. Defaults to the comment at the end of the key.">
        <input
          id="ssh-key-name"
          className={inputClass}
          value={name}
          maxLength={80}
          autoComplete="off"
          onChange={(e) => setName(e.target.value)}
        />
      </Field>
      <Field id="ssh-key-public" label="Public key" error={text}>
        <textarea
          id="ssh-key-public"
          className={`${inputClass} min-h-24 font-mono text-xs`}
          value={line}
          rows={3}
          spellCheck={false}
          autoComplete="off"
          placeholder="ssh-ed25519 AAAA… you@laptop"
          onChange={(e) => setLine(e.target.value)}
        />
      </Field>
      <div className="space-y-1.5">
        <p className="text-xs text-muted">No key yet? Create one on your computer:</p>
        <CopyBlock text="ssh-keygen -t ed25519" label="the ssh-keygen command" />
      </div>
    </FormDialog>
  );
}

function RenameDialog({ target, onClose }: { target: SshKey | null; onClose: () => void }) {
  const rename = useRenameSshKey();
  const [name, setName] = useState("");
  const [seen, setSeen] = useState<string | null>(null);
  // Start from the key's current name each time the dialog opens for a key.
  if (target && seen !== target.id) {
    setSeen(target.id);
    setName(target.name);
    rename.reset();
  }
  if (!target && seen !== null) setSeen(null);
  return (
    <FormDialog
      open={target !== null}
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
      title="Rename SSH key"
      submitLabel="Save"
      busy={rename.isPending}
      submitDisabled={name.trim() === ""}
      onSubmit={(e) => {
        e.preventDefault();
        if (!target) return;
        rename.mutate({ id: target.id, name: name.trim() }, { onSuccess: onClose });
      }}
    >
      <Field id="ssh-key-rename" label="Name" error={rename.error ? errorMessage(rename.error) : undefined}>
        <input
          id="ssh-key-rename"
          className={inputClass}
          value={name}
          maxLength={80}
          autoComplete="off"
          onChange={(e) => setName(e.target.value)}
        />
      </Field>
    </FormDialog>
  );
}

export function SshKeysView() {
  const instance = useInstance();
  const enabled = instance.data?.features.ssh === true;
  const keys = useSshKeys(enabled);
  const del = useDeleteSshKey();
  const [adding, setAdding] = useState(false);
  const [renaming, setRenaming] = useState<SshKey | null>(null);
  const [deleting, setDeleting] = useState<SshKey | null>(null);

  if (instance.isPending) return <Loading />;
  if (instance.isError) return <ErrorState error={instance.error} onRetry={() => instance.refetch()} />;
  if (!enabled) {
    return (
      <Section id="ssh-keys" title="SSH keys">
        <Notice tone="neutral">SSH access is not enabled on this Aurora instance.</Notice>
      </Section>
    );
  }
  if (keys.isPending) return <Loading />;
  if (keys.isError) return <ErrorState error={keys.error} onRetry={() => keys.refetch()} />;

  const addButton = <Button onClick={() => setAdding(true)}>Add key</Button>;
  return (
    <>
      <Section
        id="ssh-keys"
        title="SSH keys"
        description="Public keys you use to connect to your agents over SSH. Aurora checks one against a short-lived certificate each time you connect."
        actions={keys.data.length > 0 ? addButton : undefined}
      >
        {del.error ? (
          <Notice tone="danger" role="alert">
            {errorMessage(del.error)}
          </Notice>
        ) : null}
        {keys.data.length === 0 ? (
          <EmptyState
            title="No SSH keys yet"
            when="Add the public key of your computer to connect to an agent with ssh or the Aurora CLI."
            action={addButton}
          />
        ) : (
          <DataTable caption="SSH keys">
            <thead>
              <tr>
                <Th>Name</Th>
                <Th>Fingerprint</Th>
                <Th>Added</Th>
                <Th>Last used</Th>
                <Th>
                  <span className="sr-only">Actions</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {keys.data.map((k) => (
                <tr key={k.id}>
                  <Td className="break-words font-medium">{k.name}</Td>
                  <Td>
                    <div className="flex flex-wrap items-center gap-2">
                      <span title={k.fingerprint} className="break-all font-mono text-xs text-ink-2">
                        {middleTruncate(k.fingerprint)}
                      </span>
                      <CopyButton text={k.fingerprint} label={`fingerprint of ${k.name}`} />
                    </div>
                  </Td>
                  <Td className="whitespace-nowrap text-ink-2">{fmtDate(k.created_at)}</Td>
                  <Td className="whitespace-nowrap text-ink-2">{fmtDate(k.last_used_at)}</Td>
                  <Td className="text-right">
                    <div className="flex flex-wrap justify-end gap-2">
                      <Button variant="outline" size="sm" aria-label={`Rename ${k.name}`} onClick={() => setRenaming(k)}>
                        Rename
                      </Button>
                      <Button variant="outline" size="sm" aria-label={`Delete ${k.name}`} onClick={() => setDeleting(k)}>
                        Delete
                      </Button>
                    </div>
                  </Td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        )}
      </Section>
      <AddKeyDialog open={adding} onOpenChange={setAdding} />
      <RenameDialog target={renaming} onClose={() => setRenaming(null)} />
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(o) => {
          if (!o) setDeleting(null);
        }}
        title="Delete SSH key"
        description={`Delete "${deleting?.name ?? ""}"? Open SSH sessions that use this key end within 30 seconds, and you can no longer get certificates with it.`}
        confirmLabel="Delete key"
        danger
        busy={del.isPending}
        onConfirm={() => {
          if (!deleting) return;
          del.mutate(deleting.id, { onSettled: () => setDeleting(null) });
        }}
      />
    </>
  );
}
