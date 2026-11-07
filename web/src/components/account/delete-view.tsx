"use client";

import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { deleteAccount } from "@/lib/api/account";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";

export function deleteErrorMessage(err: unknown): string {
  if (err instanceof ApiError && err.code === "last_owner") {
    return "You are the last owner of an organization. Transfer ownership or delete the organization first, then try again.";
  }
  if (err instanceof ApiError && err.code === "reauthentication_required") {
    return "Enter your password to confirm.";
  }
  return errorMessage(err);
}

export function DeleteView({ onDeleted }: { onDeleted?: () => void }) {
  const [password, setPassword] = useState("");
  const router = useRouter();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setPending(true);
    setError(null);
    try {
      await deleteAccount(password || null);
      (onDeleted ?? (() => router.push("/auth/login")))();
    } catch (err) {
      setError(err);
      setPending(false);
    }
  }

  return (
    <form onSubmit={(e) => void submit(e)} noValidate className="max-w-md space-y-4">
      <h2 className="text-lg font-medium text-red-800">Delete account</h2>
      <p className="text-sm text-neutral-600">
        This permanently deletes your account and signs you out everywhere. It cannot be undone.
        Leave the password empty if your account has none (single sign-on).
      </p>
      <Field id="del-pw" label="Password">
        <input
          id="del-pw"
          type="password"
          autoComplete="current-password"
          className={inputClass}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </Field>
      {error !== null ? (
        <p role="alert" className="text-sm text-red-700">
          {deleteErrorMessage(error)}
        </p>
      ) : null}
      <Button type="submit" disabled={pending} className="bg-red-700 text-white">
        {pending ? "Deleting…" : "Delete my account"}
      </Button>
    </form>
  );
}
