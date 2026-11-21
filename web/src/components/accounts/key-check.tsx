"use client";

import type { ReactNode } from "react";

import { StatusLine, type StatusTone } from "@/components/ui/status-line";
import { type Account, type AccountsScope, useAccounts } from "@/lib/api/accounts";

type Verification = NonNullable<Account["verification"]>;

/**
 * (P5-REFINE-13 access-8, P5-REFINE-20 b1#1, C-1065) An AI key's check in words: Checking… only while the server
 * says a re-check is running; once it is over the state is final ("Not checked: …" when the AI service never
 * answered Aurora, Works after its first real answer, Failed with the reason).
 */
export function keyCheckText(v: Verification): string {
  switch (v.state) {
    case "works":
      return "Works";
    case "checking":
      return "Checking…";
    case "rejected":
      return "Failed: the provider rejected the key";
    case "unreachable":
      return "Failed: the provider could not be reached";
    default:
      return v.checked_at
        ? "Not checked: the AI service didn't answer Aurora's check. It shows Works after its first answer."
        : "Not checked yet. It shows Works after its first answer.";
  }
}

const TONE: Record<Verification["state"], StatusTone> = {
  works: "ok",
  checking: "idle",
  not_checked: "idle",
  rejected: "fail",
  unreachable: "fail",
};

/** The key's check as one status line, with the row's retry beside it when given. */
export function KeyCheckLine({ verification, action }: { verification: Verification; action?: ReactNode }) {
  return (
    <StatusLine tone={TONE[verification.state]} action={action}>
      <span data-testid="key-check">{keyCheckText(verification)}</span>
    </StatusLine>
  );
}

/**
 * A key just saved, followed live from the accounts list (the same query as its row, which polls while it is
 * checked): never a stored `pending` origin shown as "Checking…" forever. Nothing until the list has the key.
 */
export function SavedKeyCheck({ scope, secretId }: { scope: AccountsScope; secretId: string }) {
  const accounts = useAccounts(scope);
  const account = accounts.data?.items.find((a) => a.ref.type === "secret" && a.ref.id === secretId);
  if (!account?.verification) return null;
  return <KeyCheckLine verification={account.verification} />;
}
