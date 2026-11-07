"use client";

import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useEffect, useState } from "react";

import { AuthCard, Notice } from "@/components/auth/auth-card";
import { FormError } from "@/components/auth/form-error";
import { Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ApiError } from "@/lib/api/client";
import { useConfirmChannelLink } from "@/lib/api/channels";
import { useSession } from "@/lib/api/queries";
import { isUnauthorized, loginUrl } from "@/lib/auth/use-unauthorized-redirect";

import { channelError } from "./shared";

const PATH = "/link/channel";

/** Remove the single-use token from the address bar (and history entry). */
function stripToken() {
  try {
    window.history.replaceState(window.history.state, "", PATH);
  } catch {
    // ignore
  }
}

export function LinkConfirmView() {
  // Captured once so the token can leave the URL without losing it for a retry.
  const params = useSearchParams();
  const [token] = useState(() => params.get("token"));
  const session = useSession();
  const confirm = useConfirmChannelLink();

  useEffect(() => {
    if (session.isError && isUnauthorized(session.error)) {
      // The return URL still needs the token so the flow resumes after sign-in.
      window.location.assign(loginUrl(window.location.pathname + window.location.search));
    }
  }, [session.isError, session.error]);

  if (!token) {
    return (
      <AuthCard title="Link your chat account">
        <FormError message="This link is incomplete. Ask for a new one in the chat." />
      </AuthCard>
    );
  }
  if (session.isPending || (session.isError && isUnauthorized(session.error))) {
    return (
      <AuthCard title="Link your chat account">
        <Loading />
      </AuthCard>
    );
  }
  if (session.isError) {
    return (
      <AuthCard title="Link your chat account">
        <FormError error={session.error} />
      </AuthCard>
    );
  }

  if (confirm.isSuccess) {
    const l = confirm.data;
    return (
      <AuthCard title="Account linked">
        <Notice>
          <code>{l.platform_user_ref}</code> on {l.tenant_label} is now linked to {l.user.display_name}.
        </Notice>
        <Link href="/account/channels" className="text-sm underline">
          Manage chat links
        </Link>
      </AuthCard>
    );
  }

  const terminal =
    confirm.error instanceof ApiError &&
    (confirm.error.code === "link_expired" || confirm.error.code === "already_linked");

  return (
    <AuthCard title="Link your chat account">
      <p className="text-sm">
        Link your chat account to <strong>{session.data.user.display_name}</strong> ({session.data.user.email}) so
        agents know who you are in chat.
      </p>
      <FormError message={confirm.isError ? channelError(confirm.error) : undefined} />
      {!terminal ? (
        <Button
          disabled={confirm.isPending}
          onClick={() =>
            confirm.mutate(token, {
              // Single-use: drop it from the URL once the server has answered.
              onSettled: (_d, err) => {
                if (!err || err instanceof ApiError) stripToken();
              },
            })
          }
        >
          {confirm.isPending ? "Linking…" : "Confirm link"}
        </Button>
      ) : null}
    </AuthCard>
  );
}
