"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { errorMessage } from "@/lib/api/errors";
import {
  useAddTrustedKey,
  useRevokeTrustedKey,
  useTrustedKeys,
  type TrustedKey,
} from "@/lib/api/plugins";

function AddKeyForm() {
  const add = useAddTrustedKey();
  const [publisher, setPublisher] = useState("");
  const [name, setName] = useState("");
  const [key, setKey] = useState("");
  return (
    <Form
      onSubmit={(e) => {
        e.preventDefault();
        add.mutate(
          { publisher: publisher.trim(), name: name.trim(), public_key: key.trim() },
          {
            onSuccess: () => {
              setPublisher("");
              setName("");
              setKey("");
            },
          },
        );
      }}
    >
      <Field id="tk-publisher" label="Publisher">
        <input id="tk-publisher" required className={inputClass} value={publisher} onChange={(e) => setPublisher(e.target.value)} />
      </Field>
      <Field id="tk-name" label="Name">
        <input id="tk-name" required className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="tk-key" label="Public key">
        <input id="tk-key" required className={`${inputClass} font-mono`} value={key} onChange={(e) => setKey(e.target.value)} />
      </Field>
      {add.isError ? (
        <Notice tone="danger" role="alert">
          {errorMessage(add.error)}
        </Notice>
      ) : null}
      <FormActions>
        <Button type="submit" size="lg" disabled={add.isPending}>
          Add trusted key
        </Button>
      </FormActions>
    </Form>
  );
}

export function TrustedKeysView() {
  const q = useTrustedKeys();
  const revoke = useRevokeTrustedKey();
  const [target, setTarget] = useState<TrustedKey | null>(null);
  return (
    <div className="max-w-page space-y-8">
      <PageHeader title="Trusted keys" description="Publishers whose signed plugin packages this instance accepts." className="mb-6" />
      <Section id="tk-add" title="Add a trusted key" card>
        <AddKeyForm />
      </Section>
      <Section id="tk-list" title="Keys">
        {q.isPending ? <Loading /> : null}
        {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
        {q.data && q.data.items.length === 0 ? <EmptyState title="No trusted keys" /> : null}
        {q.data && q.data.items.length > 0 ? (
          <DataTable caption="Trusted keys">
            <thead>
              <tr>
                <Th>Name</Th>
                <Th>Publisher</Th>
                <Th>Added</Th>
                <Th>Status</Th>
              </tr>
            </thead>
            <tbody>
              {q.data.items.map((k) => (
                <tr key={k.id}>
                  <Td>
                    <span className="font-medium">{k.name}</span>
                    <div className="max-w-md break-all font-mono text-xs text-muted">{k.public_key}</div>
                  </Td>
                  <Td>{k.publisher}</Td>
                  <Td className="whitespace-nowrap text-ink-2">{fmtDate(k.created_at)}</Td>
                  <Td>
                    {k.revoked_at ? (
                      <Badge tone="danger">{`Revoked ${fmtDate(k.revoked_at)}`}</Badge>
                    ) : (
                      <Button size="sm" variant="danger" aria-label={`Revoke ${k.name}`} onClick={() => setTarget(k)}>
                        Revoke
                      </Button>
                    )}
                  </Td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        ) : null}
      </Section>
      <ConfirmDialog
        open={target !== null}
        onOpenChange={(o) => !o && setTarget(null)}
        title="Revoke trusted key"
        description="Revoking this key disables every package signed by it."
        confirmLabel="Revoke key"
        danger
        busy={revoke.isPending}
        onConfirm={() => target && revoke.mutate(target.id, { onSuccess: () => setTarget(null) })}
      >
        {revoke.isError ? (
          <Notice tone="danger" role="alert" className="mb-2">
            {errorMessage(revoke.error)}
          </Notice>
        ) : null}
      </ConfirmDialog>
    </div>
  );
}
