"use client";

import { useRef, useState } from "react";

import { AgentAvatar } from "@/components/agent-avatar";
import { Button } from "@/components/ui/button";
import { InlineSaved, useSaveBar } from "@/components/ui/save-bar";
import {
  avatarErrorMessage,
  avatarProblem,
  useRemoveAvatar,
  useUploadAvatar,
} from "@/lib/api/avatar";
import type { Agent } from "@/lib/api/queries";

/**
 * (P5-REFINE-12, behaviour-8) The agent's picture on the Instructions page: the round picture, a visible **Change
 * picture** button, a real **Remove** button and the limit in words. It acts at once (not on Save) and says "Saved"
 * beside itself.
 */
export function AvatarField({ agent, wsId }: { agent: Agent; wsId: string }) {
  const upload = useUploadAvatar(wsId, agent.id);
  const remove = useRemoveAvatar(wsId, agent.id);
  const input = useRef<HTMLInputElement>(null);
  const bar = useSaveBar();
  const [problem, setProblem] = useState("");
  const [preview, setPreview] = useState<string | null>(null);
  const busy = upload.isPending || remove.isPending;
  const serverError = upload.error ?? remove.error;

  async function onPick(file: File | undefined) {
    if (!file) return;
    const p = avatarProblem(file);
    setProblem(p ?? "");
    if (p) return;
    remove.reset();
    setPreview(URL.createObjectURL(file));
    try {
      await upload.mutateAsync(file);
      bar.flash();
    } catch {
      // surfaced via upload.error
    } finally {
      setPreview((u) => {
        if (u) URL.revokeObjectURL(u);
        return null;
      });
      if (input.current) input.current.value = "";
    }
  }

  return (
    <div className="flex items-start gap-4">
      <AgentAvatar name={agent.name} url={preview ?? agent.avatar_url} size={64} />
      <div className="min-w-0 space-y-2">
        <p className="text-sm font-medium text-ink">Picture</p>
        <input
          ref={input}
          id="a-avatar"
          type="file"
          accept="image/png,image/jpeg,image/webp"
          className="sr-only"
          aria-label="Avatar file"
          tabIndex={-1}
          disabled={busy}
          onChange={(e) => void onPick(e.target.files?.[0])}
        />
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" variant="outline" size="sm" disabled={busy} onClick={() => input.current?.click()}>
            {upload.isPending ? "Uploading…" : "Change picture"}
          </Button>
          {agent.avatar_url ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={busy}
              aria-label="Remove picture"
              onClick={() => {
                setProblem("");
                upload.reset();
                remove.mutate(undefined, { onSuccess: () => bar.flash() });
              }}>
              {remove.isPending ? "Removing…" : "Remove"}
            </Button>
          ) : null}
          <InlineSaved show={bar.saved} />
        </div>
        <p className="text-xs text-muted">PNG, JPG or WebP, up to 512 KiB. Shown round.</p>
        {problem || serverError ? (
          <p role="alert" className="text-xs text-danger-ink">
            {problem || avatarErrorMessage(serverError)}
          </p>
        ) : null}
      </div>
    </div>
  );
}
