"use client";

import { useInstance } from "@/lib/api/queries";

/** Shows a notice when the API reports a different version than this build. */
export function VersionBanner() {
  const built = process.env.NEXT_PUBLIC_AURORA_VERSION;
  const instance = useInstance(!!built);
  if (!built || !instance.data || instance.data.version === built) return null;
  return (
    <div role="status" className="bg-amber-100 px-4 py-2 text-center text-sm text-amber-900">
      A new version of Aurora is available.{" "}
      <button type="button" className="underline" onClick={() => window.location.reload()}>
        Reload
      </button>
    </div>
  );
}
