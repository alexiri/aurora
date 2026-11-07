"use client";

import { Suspense } from "react";

import { ConnectView } from "@/components/apps/connect-view";
import { Loading } from "@/components/states";

export default function Page() {
  return (
    <Suspense fallback={<Loading />}>
      <ConnectView />
    </Suspense>
  );
}
