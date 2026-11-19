"use client";

import { useRef, useState } from "react";

import { AgentAvatar } from "@/components/agent-avatar";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import {
  avatarErrorMessage,
  avatarProblem,
  useRemoveAvatar,
  useUploadAvatar,
} from "@/lib/api/avatar";
import type { Agent } from "@/lib/api/queries";

/** Avatar upload / preview / remove for the Instructions tab (acts immediately, not on Save). */
export function AvatarField({ agent, wsId, compact = false }: { agent: Agent; wsId: string; compact?: boolean }) {
  const upload = useUploadAvatar(wsId, agent.id);
  const remove = useRemoveAvatar(wsId, agent.id);
  const input = useRef<HTMLInputElement>(null);
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

  // (P5-POLISH-6, C3) Compact: the avatar is the button that changes it; Remove is a small link under it.
  if (compact) {
    return (
      <div className="flex shrink-0 flex-col items-center gap-1">
        <input
          ref={input}
          id="a-avatar"
          type="file"
          accept="image/png,image/jpeg,image/webp"
          className="sr-only"
          aria-label="Avatar file"
          disabled={busy}
          onChange={(e) => void onPick(e.target.files?.[0])}
        />
        <button
          type="button"
          disabled={busy}
          aria-label={agent.avatar_url ? "Change avatar" : "Upload avatar"}
          title="PNG, JPEG or WebP, up to 512 KiB"
          onClick={() => input.current?.click()}
          className="rounded-full">
          <AgentAvatar name={agent.name} url={preview ?? agent.avatar_url} size={56} />
        </button>
        {agent.avatar_url ? (
          <button
            type="button"
            disabled={busy}
            aria-label="Remove avatar"
            className="text-xs text-muted underline hover:text-ink"
            onClick={() => {
              setProblem("");
              upload.reset();
              remove.mutate();
            }}>
            {remove.isPending ? "Removing…" : "Remove"}
          </button>
        ) : null}
        {problem || serverError ? (
          <p role="alert" className="max-w-40 text-xs text-danger-ink">
            {problem || avatarErrorMessage(serverError)}
          </p>
        ) : null}
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-4">
        <AgentAvatar name={agent.name} url={preview ?? agent.avatar_url} size={64} />
        <div className="flex flex-wrap items-center gap-2">
          <input
            ref={input}
            id="a-avatar"
            type="file"
            accept="image/png,image/jpeg,image/webp"
            className="sr-only"
            aria-label="Avatar file"
            disabled={busy}
            onChange={(e) => void onPick(e.target.files?.[0])}
          />
          <Button type="button" variant="outline" disabled={busy} onClick={() => input.current?.click()}>
            {upload.isPending ? "Uploading…" : agent.avatar_url ? "Change avatar" : "Upload avatar"}
          </Button>
          {agent.avatar_url ? (
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => {
                setProblem("");
                upload.reset();
                remove.mutate();
              }}
            >
              {remove.isPending ? "Removing…" : "Remove avatar"}
            </Button>
          ) : null}
        </div>
      </div>
      <p className="text-xs text-muted">PNG, JPEG or WebP, up to 512 KiB. It is cropped to a square and resized to 256×256.</p>
      {problem || serverError ? (
        <Notice tone="danger" role="alert">
          {problem || avatarErrorMessage(serverError)}
        </Notice>
      ) : null}
    </div>
  );
}
