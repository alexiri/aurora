"use client";

import { useEffect } from "react";

import { AuthCard } from "@/components/auth/auth-card";
import { ErrorState } from "@/components/states";
import { recordError } from "@/lib/telemetry";

// (FIX-146, D-232 (i)) The App Router error boundary: a render error in any page is captured as a `browser.error`
// span (stack frames only, never the message; a no-op unless browser tracing is on) and shown with a retry.
export default function RootError({ error, reset }: { error: Error & { digest?: string }; reset: () => void }) {
  useEffect(() => {
    recordError(error);
  }, [error]);
  return (
    <AuthCard title="Something went wrong">
      <ErrorState error={new Error("This page could not be shown.")} onRetry={reset} />
    </AuthCard>
  );
}
