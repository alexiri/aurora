"use client";

import Link from "next/link";
import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { useAppAvailability, usePutAppAvailability, type AppAvailability } from "@/lib/api/apps";
import { errorMessage } from "@/lib/api/errors";

/**
 * (FIX-130) The org's **app posture**: which installed apps may be connected at all, next to the LLM and egress
 * postures. It saves through its own route (`PUT /v1/orgs/{org}/app-availability`, the same call the Apps page
 * uses for the per-app list), so it has its own Save button and never rides on "Save governance". The posture is
 * sent together with the allow-list as it currently is on the server; the Apps page does the same the other way.
 */
export function AppsPosturePanel({ orgId, orgSlug, readOnly }: { orgId: string; orgSlug: string; readOnly: boolean }) {
  const avail = useAppAvailability(orgId);
  if (avail.isPending) return <Loading />;
  if (avail.isError) return <ErrorState error={avail.error} onRetry={() => avail.refetch()} />;
  return <PostureForm orgId={orgId} orgSlug={orgSlug} current={avail.data} readOnly={readOnly} />;
}

function PostureForm({
  orgId,
  orgSlug,
  current,
  readOnly,
}: {
  orgId: string;
  orgSlug: string;
  current: AppAvailability;
  readOnly: boolean;
}) {
  const put = usePutAppAvailability(orgId);
  const [posture, setPosture] = useState<"open" | "restricted">(current.posture === "restricted" ? "restricted" : "open");
  const [saved, setSaved] = useState(false);
  const changed = posture !== current.posture;
  const choose = (p: "open" | "restricted") => {
    setPosture(p);
    setSaved(false);
  };
  return (
    <Card className="space-y-4">
      <h2 className="text-base font-semibold text-ink">Apps</h2>
      <p className="text-sm text-ink-2">Which installed apps people in this organization can connect.</p>
      <fieldset className="space-y-3" disabled={readOnly}>
        <legend className="sr-only">App posture</legend>
        <label className="flex items-start gap-2.5 text-sm text-ink">
          <input
            type="radio"
            name="app-posture"
            className="mt-0.5 size-4 accent-accent"
            checked={posture === "open"}
            onChange={() => choose("open")}
          />
          Open — every active app can be connected
        </label>
        <label className="flex items-start gap-2.5 text-sm text-ink">
          <input
            type="radio"
            name="app-posture"
            className="mt-0.5 size-4 accent-accent"
            checked={posture === "restricted"}
            onChange={() => choose("restricted")}
          />
          Restricted — only the apps listed as available can be connected
        </label>
      </fieldset>
      <p className="text-sm">
        <Link className="font-medium text-link underline" href={`/o/${orgSlug}/apps`}>
          Choose available apps →
        </Link>
      </p>
      {put.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(put.error)}
        </Notice>
      ) : null}
      {saved ? (
        <Notice tone="accent" role="status">
          App posture saved.
        </Notice>
      ) : null}
      {readOnly ? (
        <Notice role="status">Only owners and admins can change this.</Notice>
      ) : (
        <FormActions>
          <Button
            size="lg"
            disabled={put.isPending || !changed}
            onClick={() => {
              setSaved(false);
              put.mutate({ posture, allow: current.allow }, { onSuccess: () => setSaved(true) });
            }}
          >
            {put.isPending ? "Saving…" : "Save app posture"}
          </Button>
        </FormActions>
      )}
    </Card>
  );
}
