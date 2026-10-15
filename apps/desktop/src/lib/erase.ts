import type { Refusal } from "../types";

/** Human text for a refusal. */
export function refusalText(r: Refusal): string {
  switch (r.kind) {
    case "system_disk":
      return "hosts the running operating system";
    case "executable_disk":
      return "holds the running PhoinixDR executable";
    case "sessions_disk":
      return "holds the PhoinixDR sessions directory";
    case "open_source":
      return "is open for recovery in this window";
    case "mounted":
      return `has a volume mounted at ${r.mount}; unmount it first`;
    case "not_confirmed":
      return `confirmation does not match (type ${r.expected})`;
    case "no_such_partition":
      return `has no partition ${r.index}`;
    case "out_of_bounds":
      return "range lies outside the device";
    case "unsupported":
      return r.reason;
    default:
      return "refused";
  }
}

/** Rough time left from throughput. */
export function etaText(remaining: number, bytesPerSec: number): string {
  if (bytesPerSec <= 0) return "";
  const s = Math.round(remaining / bytesPerSec);
  if (s < 90) return `${s} s left`;
  if (s < 5400) return `${Math.round(s / 60)} min left`;
  return `${(s / 3600).toFixed(1)} h left`;
}
