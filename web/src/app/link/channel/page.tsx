import { Suspense } from "react";

import { LinkConfirmView } from "@/components/channels/link-confirm-view";

export default function Page() {
  return (
    <Suspense>
      <LinkConfirmView />
    </Suspense>
  );
}
