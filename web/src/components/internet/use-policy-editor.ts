"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import {
  draftFromGeneration,
  isStale,
  policyIssues,
  toDraftRequest,
  usePolicy,
  usePolicyWarnings,
  usePublish,
  usePutDraft,
  type DraftState,
  type PolicyScope,
} from "@/lib/api/policy";
import { pendingChanges } from "@/lib/internet-access";

/**
 * (P5-IA-2, C-989) The policy of one level as an editor with **one way to save**: every edit is written to the policy draft
 * as it is made (one write at a time, in order, so the draft revision never goes stale under the person's own clicks),
 * **apply** publishes the draft, **discard** writes the published generation back. `pending` counts what the draft
 * holds that is not applied; the page's bar shows it.
 */
export function usePolicyEditor(scope: PolicyScope, enabled = true) {
  const policy = usePolicy(scope, enabled);
  const warnings = usePolicyWarnings(scope, enabled);
  const put = usePutDraft(scope);
  const publish = usePublish(scope);
  const [local, setLocal] = useState<DraftState | null>(null);
  const [stale, setStale] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);

  const serverDraft = policy.data?.draft;
  const published = policy.data ? draftFromGeneration(policy.data.published) : null;
  const state = local ?? (serverDraft ? draftFromGeneration(serverDraft) : null);

  // The latest draft revision the server reported, for the write that follows an earlier one.
  const knownRevision = useRef<number | null>(null);
  const serverRevision = serverDraft?.draft_revision ?? null;
  useEffect(() => {
    if (serverRevision !== null) knownRevision.current = serverRevision;
  }, [serverRevision]);
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const inFlight = useRef(0);
  const failed = useRef(false);

  const write = useCallback(
    (next: DraftState) => {
      inFlight.current += 1;
      queue.current = queue.current.then(async () => {
        try {
          const rev = knownRevision.current;
          if (rev === null) return;
          const draft = await put.mutateAsync(toDraftRequest(next, rev, scope.kind));
          knownRevision.current = draft.draft_revision ?? rev;
          failed.current = false;
          void warnings.refetch();
        } catch (e) {
          failed.current = true;
          if (isStale(e)) setStale(true);
          else setError(e);
        } finally {
          inFlight.current -= 1;
          // The server draft is what the page shows again once every write has landed.
          if (inFlight.current === 0 && !failed.current) setLocal(null);
        }
      });
      return queue.current;
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [put.mutateAsync, scope.kind, warnings.refetch],
  );

  /** An edit: shown at once, written to the draft behind it. */
  const edit = useCallback(
    (next: DraftState) => {
      setError(null);
      setLocal(next);
      void write(next);
    },
    [write],
  );

  const pending = state && published ? pendingChanges(published, state) : 0;

  /** Publishes the draft; resolves `true` when it is applied. */
  async function apply(): Promise<boolean> {
    if (!state) return false;
    setBusy(true);
    setError(null);
    try {
      // A write that failed (or has not landed) is repeated here, so Apply always publishes what the page shows.
      await queue.current;
      if (local && (failed.current || inFlight.current > 0)) await write(local);
      const rev = knownRevision.current;
      if (rev === null || failed.current) return false;
      await publish.mutateAsync(rev);
      setLocal(null);
      void warnings.refetch();
      return true;
    } catch (e) {
      if (isStale(e)) setStale(true);
      else setError(e);
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function discard() {
    if (!policy.data) return;
    setBusy(true);
    setError(null);
    try {
      await queue.current;
      const rev = knownRevision.current;
      if (rev === null) return;
      const d = await put.mutateAsync(toDraftRequest(draftFromGeneration(policy.data.published), rev, scope.kind));
      knownRevision.current = d.draft_revision ?? rev;
      failed.current = false;
      setLocal(null);
    } catch (e) {
      if (isStale(e)) setStale(true);
      else setError(e);
    } finally {
      setBusy(false);
    }
  }

  function reload() {
    setStale(false);
    setError(null);
    failed.current = false;
    setLocal(null);
    void policy.refetch();
    void warnings.refetch();
  }

  return {
    policy,
    warnings,
    state,
    published,
    edit,
    apply,
    discard,
    reload,
    pending,
    busy: busy || put.isPending || publish.isPending,
    stale,
    error,
    issues: policyIssues(error),
  };
}
