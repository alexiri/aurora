import { errorMessage, fieldErrors } from "@/lib/api/errors";

/** Summary alert for errors that are not attributable to a single field. */
export function FormError({ error, message }: { error?: unknown; message?: string }) {
  if (!error && !message) return null;
  const hasFields = error ? Object.keys(fieldErrors(error)).length > 0 : false;
  if (hasFields && !message) return null;
  return (
    <p role="alert" className="rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800">
      {message ?? errorMessage(error)}
    </p>
  );
}
