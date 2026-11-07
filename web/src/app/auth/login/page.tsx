import { Suspense } from "react";

import { LoginView } from "@/components/auth/login-view";

export default function Page() {
  return (
    <Suspense>
      <LoginView />
    </Suspense>
  );
}
