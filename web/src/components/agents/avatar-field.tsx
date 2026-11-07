"use client";

import { useRef, useState } from "react";

import { AgentAvatar } from "@/components/agent-avatar";
import { Button } from "@/components/ui/button";
import {
  avatarErrorMessage,
  avatarProblem,
  useRemoveAvatar,
  useUploadAvatar,
} from "@/lib/api/avatar";
import type { Agent } from "@/lib/api/queries";

/** Avatar upload / preview / remove for the Instructions tab (acts immediately, not on Save). */
export function AvatarField({ agent, wsId }: { agent: Agent; wsId: string }) {
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

  return (
    <div className="space-y-2">
      <span className="block text-sm font-medium">Avatar</span>
      <div className="flex items-center gap-4">
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
          <Button type="button" disabled={busy} onClick={() => input.current?.click()}>
            {upload.isPending ? "Uploading…" : agent.avatar_url ? "Change avatar" : "Upload avatar"}
          </Button>
          {agent.avatar_url ? (
            <Button
              type="button"
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
      <p className="text-xs text-neutral-600">PNG, JPEG or WebP, up to 512 KiB. It is cropped to a square and resized to 256×256.</p>
      {problem || serverError ? (
        <p role="alert" className="text-sm text-red-700">
          {problem || avatarErrorMessage(serverError)}
        </p>
      ) : null}
    </div>
  );
}
