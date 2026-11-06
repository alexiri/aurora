import { Suspense } from "react";

import { ForgotView } from "@/components/auth/forgot-view";

export default function Page() {
  return (
    <Suspense>
      <ForgotView />
    </Suspense>
  );
}
