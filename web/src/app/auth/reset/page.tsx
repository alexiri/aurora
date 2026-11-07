import { Suspense } from "react";

import { ResetView } from "@/components/auth/reset-view";

export default function Page() {
  return (
    <Suspense>
      <ResetView />
    </Suspense>
  );
}
