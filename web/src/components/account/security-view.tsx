"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { revokeOtherSessions } from "@/lib/api/account";
import { errorMessage } from "@/lib/api/errors";

/** Phase 1 (RFE-004): there is no password to change; sign-in security is the IdP's. */
export function SecurityView() {
  const [revoking, setRevoking] = useState(false);
  const [revokeErr, setRevokeErr] = useState<unknown>(null);
  const [revoked, setRevoked] = useState<number | null>(null);

  async function signOutOthers() {
    setRevoking(true);
    setRevokeErr(null);
    setRevoked(null);
    try {
      const r = await revokeOtherSessions();
      setRevoked(r.revoked);
    } catch (err) {
      setRevokeErr(err);
    } finally {
      setRevoking(false);
    }
  }

  return (
    <div className="space-y-10">
      <section aria-labelledby="signin-h" className="max-w-md space-y-3">
        <h2 id="signin-h" className="text-lg font-medium">
          Sign-in
        </h2>
        <p className="text-sm text-neutral-600">
          You sign in through your organization&apos;s identity provider, which also handles
          your password and multi-factor authentication. Aurora stores no password.
        </p>
      </section>
      <section aria-labelledby="sess-h" className="max-w-md space-y-3">
        <h2 id="sess-h" className="text-lg font-medium">
          Sessions
        </h2>
        <p className="text-sm text-neutral-600">
          Sign out everywhere else. This browser stays signed in.
        </p>
        {revokeErr !== null ? (
          <p role="alert" className="text-sm text-red-700">
            {errorMessage(revokeErr)}
          </p>
        ) : null}
        {revoked !== null ? (
          <p role="status" className="text-sm text-green-700">
            Signed out {revoked} other {revoked === 1 ? "session" : "sessions"}.
          </p>
        ) : null}
        <Button variant="outline" disabled={revoking} onClick={() => void signOutOthers()}>
          Sign out other sessions
        </Button>
      </section>
    </div>
  );
}
