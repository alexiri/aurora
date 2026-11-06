import { Suspense } from "react";

import { CliView } from "@/components/auth/cli-view";

export default function Page() {
  return (
    <Suspense>
      <CliView />
    </Suspense>
  );
}
