import { Suspense } from "react";

import { SetupErrorView } from "@/components/auth/setup-error-view";

export default function Page() {
  return (
    <Suspense>
      <SetupErrorView />
    </Suspense>
  );
}
