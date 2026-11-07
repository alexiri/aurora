import { Suspense } from "react";

import { JoinView } from "@/components/auth/join-view";

export default function Page() {
  return (
    <Suspense>
      <JoinView />
    </Suspense>
  );
}
