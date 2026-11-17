"use client";

import Link from "next/link";

import type { Account } from "@/lib/api/accounts";
import { relativeTime } from "@/lib/api/files";

/**
 * The health line of an account row (`14 §7`). P5-UX-14 owns the wordings and the checks; until it lands the state
 * comes from the connection's status: sign-in expired, turned off, else when it was last used.
 */
export function AccountHealth({ account, reconnectHref }: { account: Pick<Account, "health">; reconnectHref?: string }) {
  const { state, last_used_at } = account.health;
  if (state === "disabled") return <p className="text-sm text-muted">Turned off</p>;
  if (state === "sign_in_expired" || state === "rejected" || state === "unreachable") {
    return (
      <p className="text-sm text-danger-ink">
        {state === "sign_in_expired" ? "Sign-in expired." : "Stopped working."}{" "}
        {reconnectHref ? (
          <Link className="font-medium underline" href={reconnectHref}>
            Reconnect
          </Link>
        ) : null}
      </p>
    );
  }
  return <p className="text-sm text-muted">{last_used_at ? `Used ${relativeTime(last_used_at)}` : "Not used yet"}</p>;
}
