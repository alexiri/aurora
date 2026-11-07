"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { changePassword, revokeOtherSessions } from "@/lib/api/account";
import { errorMessage, fieldErrors } from "@/lib/api/errors";

export function SecurityView() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [clientErr, setClientErr] = useState("");
  const [done, setDone] = useState(false);

  const [revoking, setRevoking] = useState(false);
  const [revokeErr, setRevokeErr] = useState<unknown>(null);
  const [revoked, setRevoked] = useState<number | null>(null);

  const fe = error ? fieldErrors(error) : {};

  async function submit(e: FormEvent) {
    e.preventDefault();
    setDone(false);
    setError(null);
    if (!current) return setClientErr("Enter your current password.");
    if (next.length < 8) return setClientErr("The new password must be at least 8 characters.");
    setClientErr("");
    setPending(true);
    try {
      await changePassword({ current_password: current, new_password: next });
      setDone(true);
      setCurrent("");
      setNext("");
    } catch (err) {
      setError(err);
    } finally {
      setPending(false);
    }
  }

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
      <section aria-labelledby="pw-h" className="max-w-md space-y-4">
        <h2 id="pw-h" className="text-lg font-medium">
          Change password
        </h2>
        <form onSubmit={(e) => void submit(e)} noValidate className="space-y-4">
          <Field id="pw-current" label="Current password" error={fe.current_password}>
            <input
              id="pw-current"
              type="password"
              autoComplete="current-password"
              className={inputClass}
              value={current}
              onChange={(e) => setCurrent(e.target.value)}
            />
          </Field>
          <Field id="pw-new" label="New password" error={clientErr || fe.new_password}>
            <input
              id="pw-new"
              type="password"
              autoComplete="new-password"
              className={inputClass}
              value={next}
              onChange={(e) => setNext(e.target.value)}
            />
          </Field>
          {error !== null && Object.keys(fe).length === 0 ? (
            <p role="alert" className="text-sm text-red-700">
              {errorMessage(error)}
            </p>
          ) : null}
          {done ? (
            <p role="status" className="text-sm text-green-700">
              Password changed.
            </p>
          ) : null}
          <Button type="submit" disabled={pending}>
            {pending ? "Changing…" : "Change password"}
          </Button>
        </form>
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
