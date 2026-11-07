"use client";

import { ErrorState, Loading } from "@/components/states";
import { useInstance } from "@/lib/api/queries";

import { AuthCard, Notice } from "./auth-card";

export function SetupErrorView() {
  const instance = useInstance();
  return (
    <AuthCard title="Instance setup problems">
      {instance.isPending ? (
        <Loading />
      ) : instance.isError ? (
        <ErrorState error={instance.error} onRetry={() => instance.refetch()} />
      ) : instance.data.setup_errors.length === 0 ? (
        <Notice>No setup problems were reported.</Notice>
      ) : (
        <>
          <p className="text-sm">
            This instance is not fully configured. An administrator needs to fix the following:
          </p>
          <ul className="list-disc space-y-1 pl-5 text-sm">
            {instance.data.setup_errors.map((e) => (
              <li key={e}>{e}</li>
            ))}
          </ul>
        </>
      )}
    </AuthCard>
  );
}
