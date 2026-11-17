"use client";

import { useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { ScopeLadder, type LadderAbove } from "@/components/ui/scope-ladder";
import { errorMessage } from "@/lib/api/errors";
import { usePutRetentionLimits, useRetention } from "@/lib/api/org";
import { whoCanFromContacts } from "@/lib/who-can";

type Fields = { conversations: string; files: string; quota: string };
type FieldErrors = Partial<Record<keyof Fields, string>>;

/** Whole numbers only; empty is "not set" (`undefined`); a number outside `min..max` is `"invalid"`. */
function toNumber(t: string, min: number, max: number): number | undefined | "invalid" {
  const v = t.trim();
  if (v === "") return undefined;
  const n = /^\d+$/.test(v) ? Number(v) : NaN;
  return n >= min && n <= max ? n : "invalid";
}

/**
 * Data › Retention of an organization (C-969, D-339): one form with the conversation days, the file days and the
 * storage quota, under Administration's maximums as ladder rows. Both windows and the quota go in one request.
 */
export function OrgRetentionForm({ orgId }: { orgId: string }) {
  const current = useRetention(orgId);
  const put = usePutRetentionLimits(orgId);
  const [edit, setEdit] = useState<Partial<Fields>>({});
  const [errors, setErrors] = useState<FieldErrors>({});
  const [saved, setSaved] = useState(false);
  if (current.isPending) return <Loading />;
  if (current.isError) return <ErrorState error={current.error} onRetry={() => current.refetch()} />;
  const d = current.data;
  const convMax = d.conversations_days_max ?? null;
  const filesMax = d.attachments_days_max ?? null;
  const quotaMax = d.attachment_quota_mib_max ?? null;
  const value: Fields = {
    conversations: edit.conversations ?? d.conversations_days?.toString() ?? "",
    files: edit.files ?? d.attachments_days?.toString() ?? "",
    quota: edit.quota ?? d.attachment_quota_mib?.toString() ?? "",
  };
  const dirty = Object.keys(edit).length > 0;
  const who = whoCanFromContacts("instance_admin", undefined, "change_this");
  const above: LadderAbove[] = [];
  if (convMax !== null) above.push({ label: "Conversations", value_display: `At most ${convMax} days without activity`, source: "instance", who_can: who });
  if (filesMax !== null) above.push({ label: "Files", value_display: `At most ${filesMax} days`, source: "instance", who_can: who });
  if (quotaMax !== null) above.push({ label: "Storage for files", value_display: `At most ${quotaMax} MiB`, source: "instance", who_can: who });

  const change = (k: keyof Fields) => (e: { target: { value: string } }) => {
    setEdit((cur) => ({ ...cur, [k]: e.target.value }));
    setSaved(false);
  };

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const conv = toNumber(value.conversations, 7, 3650);
    const files = toNumber(value.files, 7, 3650);
    const quota = toNumber(value.quota, 0, 1 << 30);
    const err: FieldErrors = {};
    if (conv === "invalid") err.conversations = "Enter a whole number of days from 7 to 3650, or leave empty to keep conversations.";
    else if (conv !== undefined && convMax !== null && conv > convMax) err.conversations = `Administration allows at most ${convMax} days.`;
    if (files === "invalid") err.files = "Enter a whole number of days from 7 to 3650, or leave empty to keep files.";
    else if (files !== undefined && filesMax !== null && files > filesMax) err.files = `Administration allows at most ${filesMax} days.`;
    if (quota === "invalid") err.quota = "Enter a whole number of MiB, or leave empty for the default.";
    else if (quota !== undefined && quotaMax !== null && (quota === 0 || quota > quotaMax)) err.quota = `Administration allows at most ${quotaMax} MiB.`;
    setErrors(err);
    if (Object.keys(err).length > 0) return;
    put.mutate(
      {
        conversations_days: typeof conv === "number" ? conv : null,
        attachments_days: typeof files === "number" ? files : null,
        attachment_quota_mib: typeof quota === "number" ? quota : null,
      },
      {
        onSuccess: () => {
          setEdit({});
          setSaved(true);
        },
      },
    );
  }

  const serverErr = put.error ? errorMessage(put.error) : undefined;
  return (
    <ScopeLadder
      above={above}
      here={
        <Form onSubmit={submit} noValidate aria-label="Retention" className="max-w-md">
          <Field
            id="retention-days"
            label="Delete conversations after (days without activity)"
            hint={`${convMax !== null ? `Between 7 and ${convMax} days.` : "Between 7 and 3650 days."} Messages, attachments and the agent's stored session go. Empty keeps them until someone deletes them.`}
            error={errors.conversations}
          >
            <input id="retention-days" inputMode="numeric" className={inputClass} value={value.conversations} onChange={change("conversations")} />
          </Field>
          <Field
            id="attachments-days"
            label="Delete files after (days)"
            hint={`Expired files show “Expired by retention” in the conversation. ${filesMax !== null ? `Between 7 and ${filesMax} days.` : "Between 7 and 3650 days."} Empty keeps them.`}
            error={errors.files}
          >
            <input id="attachments-days" inputMode="numeric" className={inputClass} value={value.files} onChange={change("files")} />
          </Field>
          <Field
            id="attachment-quota"
            label="Storage for files (MiB)"
            hint={`Empty uses the default (${d.attachment_quota_mib_effective ?? "unlimited"} MiB in force).${quotaMax !== null ? ` At most ${quotaMax} MiB.` : " 0 means unlimited."}`}
            error={errors.quota}
          >
            <input id="attachment-quota" inputMode="numeric" className={inputClass} value={value.quota} onChange={change("quota")} />
          </Field>
          {serverErr ? (
            <Notice tone="danger" role="alert">
              {serverErr}
            </Notice>
          ) : null}
          {saved ? (
            <Notice tone="accent" role="status">
              Retention saved.
            </Notice>
          ) : null}
          <FormActions>
            <Button
              type="button"
              variant="outline"
              size="lg"
              disabled={!dirty || put.isPending}
              onClick={() => {
                setEdit({});
                setErrors({});
              }}
            >
              Discard changes
            </Button>
            <Button type="submit" size="lg" disabled={put.isPending}>
              {put.isPending ? "Saving…" : "Save retention"}
            </Button>
          </FormActions>
        </Form>
      }
    />
  );
}
