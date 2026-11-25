"use client";

// (FIX-236, D-478, C-1164) "Open emergency read access" on the manage page, for organization owners
// only: a reason (10–500 characters, checked here before anything is sent), a duration (1–24 hours,
// 4 by default — it can never be extended), then the confirmation ladder's danger step. The people
// of the workspace and the organization's other owners are told at once; on success the owner goes
// to the workspace, where the read-only emergency mode waits. Refusals (403 org_owner_required,
// 409 already_has_access / emergency_access_active, 422 on /reason or /hours) surface their notices.

import { useParams, useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { ErrorNotice } from "@/components/ui/notice";
import { Field, inputClass } from "@/components/ui/field";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/select";
import { useOpenEmergencyAccess } from "@/lib/api/emergency";
import { wsRoutes } from "@/lib/routes";

/** The brief's duration choices, in hours; 4 is the default. */
export const EMERGENCY_HOURS = [1, 2, 4, 8, 24] as const;
export const EMERGENCY_DEFAULT_HOURS = 4;

const REASON_MIN = 10;
const REASON_MAX = 500;

/** The reason's one-line rule, in words (the server's 422 says the same thing). */
export function emergencyReasonError(reason: string): string | null {
  const n = reason.trim().length;
  if (n === 0) return "Say why you need to read this workspace.";
  if (n < REASON_MIN) return `A reason takes at least ${REASON_MIN} characters.`;
  if (n > REASON_MAX) return `Keep the reason within ${REASON_MAX} characters.`;
  return null;
}

export function emergencyHoursOptions(): { value: string; label: string }[] {
  return EMERGENCY_HOURS.map((h) => ({ value: String(h), label: `${h} hour${h === 1 ? "" : "s"}${h === EMERGENCY_DEFAULT_HOURS ? " (default)" : ""}` }));
}

/**
 * The trigger button (the caller decides where it stands and who sees it), the form step and the
 * danger step. `onOpened` (default: go to the workspace) runs after the 201.
 */
export function OpenEmergencyAccess({
  wsId,
  wsName,
  onOpened,
}: {
  wsId: string;
  wsName: string;
  /** Runs after the grant is opened; default navigates to the workspace. */
  onOpened?: () => void;
}) {
  const router = useRouter();
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const [open, setOpen] = useState(false);
  // The two steps: the form first, then the danger confirmation with the consequence.
  const [reason, setReason] = useState("");
  const [hours, setHours] = useState(String(EMERGENCY_DEFAULT_HOURS));
  const [reasonErr, setReasonErr] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const openMutation = useOpenEmergencyAccess(wsId);
  const go = onOpened ?? (() => router.push(wsRoutes(org ?? "", ws ?? "").base));

  const close = () => {
    setOpen(false);
    setConfirming(false);
    setReason("");
    setReasonErr(null);
    setHours(String(EMERGENCY_DEFAULT_HOURS));
    openMutation.reset();
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const err = emergencyReasonError(reason);
    setReasonErr(err);
    if (err) return;
    setConfirming(true);
  };

  const confirm = () => {
    openMutation.mutate(
      { reason: reason.trim(), hours: Number(hours) },
      { onSuccess: () => { close(); go(); } },
    );
  };

  return (
    <>
      <Button variant="outline" size="sm" onClick={() => setOpen(true)} data-testid="open-emergency">
        Open emergency read access…
      </Button>

      {confirming ? (
        // The confirmation ladder's danger step (the second rung): the consequence in plain words.
        // Cancel goes back to the form step with the reason kept.
        <ConfirmDialog
          open={open}
          onOpenChange={(o) => (o ? undefined : setConfirming(false))}
          title={`Open emergency read access to ${wsName}?`}
          description={`Everyone in ${wsName} and the organization's other owners will be told, with your reason. Everything you open is recorded. You can read, not act.`}
          confirmLabel={`Open emergency read access to ${wsName}`}
          cancelLabel="Back"
          danger
          busy={openMutation.isPending}
          onConfirm={confirm}
        >
          <p className="text-sm text-ink-2">
            For {hours} hour{hours === "1" ? "" : "s"}: until you or another owner ends it, or the time runs out.
          </p>
          {/* The refusal in the server's own words (403/409/422), announced as an alert; going back shows it in the form. */}
          {openMutation.isError ? <ErrorNotice error={openMutation.error} /> : null}
        </ConfirmDialog>
      ) : (
        <FormDialog
          open={open}
          onOpenChange={(o) => (o ? undefined : close())}
          title="Open emergency read access"
          description={`Why do you need to read ${wsName}? The reason is shown to its people and the organization's other owners.`}
          submitLabel="Continue"
          danger
          busy={openMutation.isPending}
          onSubmit={submit}
          error={openMutation.error}
        >
          <div className="space-y-4">
            <Field id="emergency-reason" label="Reason" error={reasonErr ?? undefined} hint={`Between ${REASON_MIN} and ${REASON_MAX} characters.`}>
              <textarea
                id="emergency-reason"
                className={`${inputClass} resize-y`}
                rows={4}
                value={reason}
                maxLength={REASON_MAX}
                onChange={(e) => {
                  setReason(e.target.value);
                  if (reasonErr) setReasonErr(null);
                }}
              />
            </Field>
            <Field id="emergency-hours" label="How long" hint="It cannot be extended. A new grant opens separately, and tells everyone again.">
              <Select id="emergency-hours" options={emergencyHoursOptions()} value={hours} onChange={setHours} />
            </Field>
          </div>
        </FormDialog>
      )}
    </>
  );
}
