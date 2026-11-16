import type { ReactNode } from "react";

import { WhoCan } from "@/components/ui/who-can";
import type { WhoCanValue } from "@/lib/who-can";

/**
 * (C-961, C-981) A page's or list's main action, in the place the page puts it. A person who can use it sees the
 * action; a person who cannot sees who can, in the same place ("Only workspace owners can add an account. Ask Bob
 * Builder."), never a dead button and never an empty gap.
 */
export function MainAction({ canAct, whoCan, children }: { canAct: boolean; whoCan: WhoCanValue; children: ReactNode }) {
  if (canAct) return <>{children}</>;
  return <WhoCan value={whoCan} lead className="max-w-md text-right max-sm:text-left" />;
}
