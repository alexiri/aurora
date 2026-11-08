"use client";

import { useMutation, useQuery } from "@tanstack/react-query";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ApiError } from "@/lib/api/client";
import { acceptInvitation, previewInvitation } from "@/lib/api/pending";
import { useSession } from "@/lib/api/queries";
import { isUnauthorized } from "@/lib/auth/use-unauthorized-redirect";

import { AuthCard, Notice } from "./auth-card";
import { FormError } from "./form-error";

const STATUS_TEXT: Record<string, string> = {
  expired: "This invitation has expired. Ask for a new one.",
  cancelled: "This invitation was cancelled.",
  accepted: "This invitation has already been accepted.",
};

export function JoinView() {
  const token = useSearchParams().get("token");
  const router = useRouter();
  const preview = useQuery({
    queryKey: ["invitation-preview", token],
    queryFn: () => previewInvitation(token!),
    enabled: !!token,
    retry: false,
  });
  const session = useSession();
  const accept = useMutation({
    mutationFn: () => acceptInvitation(token!),
    onSuccess: (r) => router.replace(`/o/${r.org.slug}`),
  });

  if (!token) {
    return (
      <AuthCard title="Join an organization">
        <FormError message="This invitation link is incomplete." />
      </AuthCard>
    );
  }
  if (preview.isPending || session.isPending) {
    return (
      <AuthCard title="Join an organization">
        <Loading />
      </AuthCard>
    );
  }
  if (preview.isError) {
    const missing = preview.error instanceof ApiError && preview.error.status === 404;
    return (
      <AuthCard title="Join an organization">
        {missing && session.data ? (
          // Phase 1 (RFE-004): a first sign-in accepts the pending invitation for the verified
          // e-mail, so the link may simply have been used by that sign-in.
          <>
            <Notice>
              This invitation is no longer pending. If you just signed in with the invited
              address, you have already joined.
            </Notice>
            <Button asChild size="lg" className="w-full">
              <Link href="/">Open Aurora</Link>
            </Button>
          </>
        ) : missing ? (
          <FormError message="This invitation link is not valid." />
        ) : (
          <ErrorState error={preview.error} onRetry={() => preview.refetch()} />
        )}
      </AuthCard>
    );
  }

  const inv = preview.data;
  const title = `Join ${inv.org.name}`;
  if (inv.status !== "pending") {
    return (
      <AuthCard title={title}>
        <FormError message={STATUS_TEXT[inv.status] ?? "This invitation is no longer valid."} />
      </AuthCard>
    );
  }

  const signedIn = !!session.data;
  const signedOut = isUnauthorized(session.error);
  const joinPath = `/join?token=${encodeURIComponent(token)}`;
  const err = accept.error;

  return (
    <AuthCard title={title}>
      <p className="text-sm">
        {inv.inviter_name ? `${inv.inviter_name} invited` : "You were invited"}{" "}
        <strong>{inv.email}</strong> to join <strong>{inv.org.name}</strong> as {inv.role}.
      </p>
      {signedIn ? (
        <>
          {session.data.user.email.toLowerCase() !== inv.email.toLowerCase() ? (
            <Notice>
              You are signed in as {session.data.user.email}, but this invitation was sent to{" "}
              {inv.email}.
            </Notice>
          ) : null}
          <FormError error={err} />
          <Button size="lg" className="w-full" disabled={accept.isPending} onClick={() => accept.mutate()}>
            {accept.isPending ? "Joining…" : "Accept invitation"}
          </Button>
        </>
      ) : signedOut ? (
        <Button asChild size="lg" className="w-full">
          <Link href={`/auth/login?next=${encodeURIComponent(joinPath)}`}>
            Sign in to accept
          </Link>
        </Button>
      ) : (
        <ErrorState error={session.error} onRetry={() => session.refetch()} />
      )}
    </AuthCard>
  );
}
