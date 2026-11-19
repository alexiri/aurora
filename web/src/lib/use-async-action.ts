"use client";

import { useState } from "react";

/**
 * A one-shot async action with its own pending and error state, for the destructive forms that leave the page on
 * success: `run` clears the error, awaits `action`, and on success calls `onDone` (pending stays set while the page
 * goes away); a failure keeps the error and re-enables the button.
 */
export function useAsyncAction(action: () => Promise<unknown>, onDone: () => void) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  async function run() {
    setPending(true);
    setError(null);
    try {
      await action();
      onDone();
    } catch (err) {
      setError(err);
      setPending(false);
    }
  }
  return { pending, error, run };
}
