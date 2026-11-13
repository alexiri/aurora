"use client";

import { useMutation, useQuery } from "@tanstack/react-query";
import { usePathname, useSearchParams } from "next/navigation";
import { useEffect, useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { approveCli, getPendingCli } from "@/lib/api/auth";
import { useSession } from "@/lib/api/queries";
import { isUnauthorized, loginUrl } from "@/lib/auth/use-unauthorized-redirect";

import { AuthCard, Notice } from "./auth-card";
import { FormError } from "./form-error";

export function CliView() {
  const params = useSearchParams();
  const pathname = usePathname();
  const session = useSession();
  const [code, setCode] = useState(params.get("code") ?? "");
  const [workspaceId, setWorkspaceId] = useState("");
  const m = useMutation({
    mutationFn: (workspace_id: string) =>
      approveCli({ user_code: code.trim().toUpperCase(), workspace_id }),
  });

  // (FIX-108) What the code is asking for, once it has the shape of a code.
  const normalized = code.trim().toUpperCase();
  const pending = useQuery({
    queryKey: ["cli-login-pending", normalized],
    queryFn: () => getPendingCli(normalized),
    enabled: session.isSuccess && /^[A-Z0-9]{4}-?[A-Z0-9]{4}$/.test(normalized),
    retry: false,
  });

  const unauth = isUnauthorized(session.error);
  useEffect(() => {
    if (unauth) {
      const qs = params.toString();
      window.location.assign(loginUrl(qs ? `${pathname}?${qs}` : pathname));
    }
  }, [unauth, params, pathname]);

  if (session.isPending || unauth) {
    return (
      <AuthCard title="Authorize the Aurora CLI">
        <Loading />
      </AuthCard>
    );
  }
  if (session.isError) {
    return (
      <AuthCard title="Authorize the Aurora CLI">
        <ErrorState error={session.error} onRetry={() => session.refetch()} />
      </AuthCard>
    );
  }

  const workspaces = session.data.memberships
    .filter((ms) => ms.status === "active")
    .flatMap((ms) => ms.workspaces.map((w) => ({ ...w, org: ms.org.name })));
  const selected = workspaceId || workspaces[0]?.id || "";

  if (m.isSuccess) {
    return (
      <AuthCard title="CLI authorized">
        <Notice>You can return to your terminal now.</Notice>
      </AuthCard>
    );
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    m.mutate(selected);
  }

  return (
    <AuthCard title="Authorize the Aurora CLI">
      {workspaces.length === 0 ? (
        <Notice>You do not have access to any workspace yet.</Notice>
      ) : (
        <form onSubmit={submit} noValidate className="space-y-4">
          <p className="text-sm">
            Confirm that the code below matches the one shown in your terminal, and choose the
            workspace the CLI will act in.
          </p>
          <FormError error={m.error} />
          <Field id="code" label="Code">
            <input
              id="code"
              autoComplete="off"
              required
              className={`${inputClass} font-mono tracking-widest`}
              value={code}
              placeholder="ABCD-EFGH"
              onChange={(e) => setCode(e.target.value)}
            />
          </Field>
          <Field id="workspace" label="Workspace">
            <select
              id="workspace"
              className={inputClass}
              value={selected}
              onChange={(e) => setWorkspaceId(e.target.value)}
            >
              {workspaces.map((w) => (
                <option key={w.id} value={w.id}>
                  {w.org} / {w.name}
                </option>
              ))}
            </select>
          </Field>
          {pending.data ? (
            <div className="space-y-2" data-testid="cli-pending">
              <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 rounded-control border border-line bg-subtle p-3 text-sm">
                <dt className="text-ink-2">Requested by</dt>
                <dd className="min-w-0 break-words font-mono">{pending.data.hostname}</dd>
                <dt className="text-ink-2">Started</dt>
                <dd>{new Date(pending.data.requested_at).toLocaleString()}</dd>
                <dt className="text-ink-2">Address</dt>
                <dd className="font-mono">{pending.data.address ?? "unknown"}</dd>
              </dl>
              <Notice>
                Approving creates an API key that acts as you, in the workspace you choose
                {pending.data.key_expires_in_days
                  ? `, and expires in ${pending.data.key_expires_in_days} days`
                  : ", and does not expire"}
                . The host name is whatever the program reported; only approve a login you started
                yourself.
              </Notice>
            </div>
          ) : pending.isError ? (
            <p role="alert" className="text-sm text-danger-ink">
              This code was not found or has expired. Start again from your terminal.
            </p>
          ) : null}
          <Button type="submit" size="lg" className="w-full" disabled={m.isPending || !code.trim()}>
            {m.isPending ? "Authorizing…" : "Authorize"}
          </Button>
        </form>
      )}
    </AuthCard>
  );
}
