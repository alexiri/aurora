"use client";

import { useMutation } from "@tanstack/react-query";
import { usePathname, useSearchParams } from "next/navigation";
import { useEffect, useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { approveCli } from "@/lib/api/auth";
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
          <Button type="submit" className="w-full" disabled={m.isPending || !code.trim()}>
            {m.isPending ? "Authorizing…" : "Authorize"}
          </Button>
        </form>
      )}
    </AuthCard>
  );
}
