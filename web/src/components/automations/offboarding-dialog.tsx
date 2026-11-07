"use client";

import { useState, type ReactNode } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { errorMessage } from "@/lib/api/errors";
import type { Offboarding, ReassignTarget } from "@/lib/api/automations";

import { OffboardingPrompt } from "./owner-controls";

/**
 * Confirmation that first shows what removing or suspending a person would disable and offers
 * to reassign it (RFE-021). `reassign_to` goes out in the same request as the removal, so both
 * commit or roll back together.
 */
export function OffboardingDialog({
  open,
  title,
  description,
  who,
  confirmLabel,
  query,
  candidates,
  busy,
  error,
  onConfirm,
  onClose,
  extra,
}: {
  open: boolean;
  title: string;
  description: string;
  who: string;
  confirmLabel: string;
  query: { isPending: boolean; isError: boolean; error: unknown; data?: Offboarding; refetch: () => unknown };
  candidates: { id: string; label: string }[];
  busy: boolean;
  error?: unknown;
  onConfirm: (reassignTo: ReassignTarget | undefined) => void;
  onClose: () => void;
  extra?: ReactNode;
}) {
  const [target, setTarget] = useState<ReassignTarget | "">("");
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent title={title} description={description}>
        <div className="space-y-3">
          {query.isPending ? <Loading /> : null}
          {query.isError ? <ErrorState error={query.error} onRetry={() => void query.refetch()} /> : null}
          {query.data ? (
            <OffboardingPrompt who={who} data={query.data} candidates={candidates} target={target} onTarget={setTarget} />
          ) : null}
          {extra}
          {error ? (
            <p role="alert" className="text-sm text-red-700">
              {errorMessage(error)}
            </p>
          ) : null}
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button disabled={busy || query.isPending} onClick={() => onConfirm(target || undefined)}>
              {confirmLabel}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
