import { Suspense } from "react";

import { SignupView } from "@/components/auth/signup-view";

export default function Page() {
  return (
    <Suspense>
      <SignupView />
    </Suspense>
  );
}
