import { Notice } from "@/components/ui/notice";
import { errorMessage, fieldErrors } from "@/lib/api/errors";

/** Summary alert for errors that are not attributable to a single field. */
export function FormError({ error, message }: { error?: unknown; message?: string }) {
  if (!error && !message) return null;
  const hasFields = error ? Object.keys(fieldErrors(error)).length > 0 : false;
  if (hasFields && !message) return null;
  return (
    <Notice tone="danger" role="alert">
      {message ?? errorMessage(error)}
    </Notice>
  );
}
