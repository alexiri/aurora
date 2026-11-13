import { Notice } from "./notice";

/**
 * Shown when a list hook stopped at its paging ceiling (`truncated` from `fetchAllPages`): the screen says so rather
 * than pretending the list is complete. `what` is the plural noun, e.g. "organizations".
 */
export function MoreNotShown({ what, narrow }: { what: string; narrow?: string }) {
  return (
    <Notice role="status">
      More {what} exist than are shown here.{narrow ? ` ${narrow}` : ""}
    </Notice>
  );
}
