"use client";

import { useSearchParams } from "next/navigation";
import { Suspense } from "react";

import { Notice } from "@/components/ui/notice";
import { reauthErrorMessage } from "@/lib/api/errors";

function ReauthErrorNoticeInner() {
  const code = useSearchParams().get("reauth_error");
  if (!code) return null;
  return (
    <Notice tone="danger" role="alert">
      {reauthErrorMessage(code)}
    </Notice>
  );
}

/**
 * Shows why a provider re-authentication failed: the API returns to the page that started it with
 * `?reauth_error=<code>`. Only mapped texts are rendered, never the raw query value.
 */
export function ReauthErrorNotice() {
  return (
    <Suspense fallback={null}>
      <ReauthErrorNoticeInner />
    </Suspense>
  );
}
