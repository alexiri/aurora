"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
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
    <div className="space-y-8">
      <Section id="signin" title="Sign-in" card>
        <p className="text-sm text-ink-2">
          You sign in through your organization&apos;s identity provider, which also handles
          your password and multi-factor authentication. Aurora stores no password.
        </p>
      </Section>
      <Section id="sess" title="Sessions" description="Sign out everywhere else. This browser stays signed in." card>
        <div className="space-y-3">
          {revokeErr !== null ? (
            <Notice tone="danger" role="alert">
              {errorMessage(revokeErr)}
            </Notice>
          ) : null}
          {revoked !== null ? (
            <Notice tone="accent" role="status">
              Signed out {revoked} other {revoked === 1 ? "session" : "sessions"}.
            </Notice>
          ) : null}
          <Button variant="outline" disabled={revoking} onClick={() => void signOutOthers()}>
            Sign out other sessions
          </Button>
        </div>
      </Section>
    </div>
  );
}
