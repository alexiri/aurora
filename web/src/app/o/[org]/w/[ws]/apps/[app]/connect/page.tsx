"use client";

import { useParams, useRouter, useSearchParams } from "next/navigation";
import { Suspense, useEffect } from "react";

import { Loading } from "@/components/states";
import { connectDeepLink } from "@/lib/api/apps";

/**
 * Links already sent (the agent's connection request, I-29) point here. Connecting is a dialog on the Apps tab now
 * (D-272, C-842), so this route only redirects to the deep link; `?reconnect=<id>` opens the Reconnect dialog.
 */
function Redirect() {
  const { org, ws, app } = useParams<{ org: string; ws: string; app: string }>();
  const reconnect = useSearchParams().get("reconnect");
  const router = useRouter();
  useEffect(() => {
    router.replace(connectDeepLink(org, ws, decodeURIComponent(app), reconnect));
  }, [router, org, ws, app, reconnect]);
  return <Loading />;
}

export default function Page() {
  return (
    <Suspense fallback={<Loading />}>
      <Redirect />
    </Suspense>
  );
}
