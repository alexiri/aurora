// The sampling rule of C-732 (7), identical to `aurora_core::telemetry::{sample_threshold, rule_sampled}`:
// a trace id is sampled when its rightmost 7 bytes (hex chars 18..32), read big-endian, are
// below `floor(ratio × 2^56)`. Both sides are tested against crates/aurora-core/tests/fixtures/sampling-vectors.json.

const ONE = 1n << 56n;

/** `floor(ratio × 2^56)`; out-of-range ratios clamp. The product of a double and 2^56 is exact. */
export function sampleThreshold(ratio: number): bigint {
  if (Number.isNaN(ratio) || ratio <= 0) return 0n;
  if (ratio >= 1) return ONE;
  return BigInt(Math.floor(ratio * 2 ** 56));
}

/** Whether `traceId` (32 lowercase hex characters) is sampled at `threshold`. */
export function ruleSampled(traceId: string, threshold: bigint): boolean {
  if (!/^[0-9a-f]{32}$/.test(traceId)) return false;
  return BigInt(`0x${traceId.slice(18)}`) < threshold;
}
