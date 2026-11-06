import { Suspense } from "react";

import { VerifyView } from "@/components/auth/verify-view";

export default function Page() {
  return (
    <Suspense>
      <VerifyView />
    </Suspense>
  );
}
