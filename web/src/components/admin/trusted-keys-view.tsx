"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
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
    <form
      className="grid max-w-xl gap-3"
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
        <input id="tk-key" required className={inputClass} value={key} onChange={(e) => setKey(e.target.value)} />
      </Field>
      {add.isError ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(add.error)}
        </p>
      ) : null}
      <div>
        <Button type="submit" disabled={add.isPending}>
          Add trusted key
        </Button>
      </div>
    </form>
  );
}

export function TrustedKeysView() {
  const q = useTrustedKeys();
  const revoke = useRevokeTrustedKey();
  const [target, setTarget] = useState<TrustedKey | null>(null);
  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold">Trusted keys</h1>
      <AddKeyForm />
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {q.data && q.data.items.length === 0 ? <EmptyState title="No trusted keys" /> : null}
      {q.data && q.data.items.length > 0 ? (
        <table className="w-full text-left text-sm">
          <thead className="text-neutral-500">
            <tr>
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Publisher</th>
              <th className="py-2 pr-4 font-medium">Added</th>
              <th className="py-2 font-medium">Status</th>
            </tr>
          </thead>
          <tbody>
            {q.data.items.map((k) => (
              <tr key={k.id} className="border-t border-neutral-200">
                <td className="py-2 pr-4">
                  {k.name}
                  <div className="break-all font-mono text-xs text-neutral-500">{k.public_key}</div>
                </td>
                <td className="py-2 pr-4">{k.publisher}</td>
                <td className="py-2 pr-4">{fmtDate(k.created_at)}</td>
                <td className="py-2">
                  {k.revoked_at ? (
                    `Revoked ${fmtDate(k.revoked_at)}`
                  ) : (
                    <Button
                      size="sm"
                      variant="outline"
                      aria-label={`Revoke ${k.name}`}
                      onClick={() => setTarget(k)}
                    >
                      Revoke
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
      <Dialog open={target !== null} onOpenChange={(o) => !o && setTarget(null)}>
        <DialogContent
          title="Revoke trusted key"
          description="Revoking this key disables every package signed by it."
        >
          {revoke.isError ? (
            <p role="alert" className="mb-2 text-sm text-red-700">
              {errorMessage(revoke.error)}
            </p>
          ) : null}
          <div className="flex gap-2">
            <Button variant="outline" onClick={() => setTarget(null)}>
              Cancel
            </Button>
            <Button
              disabled={revoke.isPending}
              onClick={() => target && revoke.mutate(target.id, { onSuccess: () => setTarget(null) })}
            >
              Revoke key
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
