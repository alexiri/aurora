import { Suspense } from "react";

import { WelcomeView } from "@/components/onboarding/welcome-view";

export default function WelcomePage() {
  return (
    <Suspense>
      <WelcomeView />
    </Suspense>
  );
}
