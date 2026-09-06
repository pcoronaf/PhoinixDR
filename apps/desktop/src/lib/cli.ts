import type { ScanRequest, VolumeRange } from "../types";

/** Quotes an argument for a shell when it contains spaces or special characters. */
export function shellQuote(arg: string): string {
  if (arg.length > 0 && /^[\w./:\\~-]+$/.test(arg)) return arg;
  return `"${arg.replace(/"/g, '\\"')}"`;
}

/** The volume-selection flags of a command line, from an explicit range or a partition index. */
export function sourceFlags(partition: number | null, volume: VolumeRange | null | undefined): string[] {
  if (volume) return ["--at", String(volume.offset), "--length", String(volume.length)];
  if (partition !== null) return ["--partition", String(partition)];
  return [];
}

/** The `phoinix scan` command line equivalent to a desktop scan request. */
export function scanCommand(r: ScanRequest): string {
  const parts = ["phoinix", "scan", shellQuote(r.source), ...sourceFlags(r.partition, r.volume)];
  if (r.mode === "deep") {
    parts.push("--deep");
    if (r.carve.whole_volume) parts.push("--carve-all");
    if (r.carve.types.length > 0) parts.push("--carve-types", r.carve.types.join(","));
    if (r.carve.min_size > 0) parts.push("--carve-min-size", String(r.carve.min_size));
    if (r.carve.alignment > 0 && r.carve.alignment !== 512) parts.push("--carve-align", String(r.carve.alignment));
  }
  if (!r.examine_content) parts.push("--no-content");
  return parts.join(" ");
}

/** The `phoinix explain` command line for one candidate reference. */
export function explainCommand(source: string, partition: number | null, volume: VolumeRange | null | undefined, reference: string): string {
  return ["phoinix", "explain", shellQuote(source), ...sourceFlags(partition, volume), reference].join(" ");
}

/** The `phoinix recover` command line for candidate references. */
export function recoverCommand(source: string, partition: number | null, volume: VolumeRange | null | undefined, references: string[], destination: string | null): string {
  return ["phoinix", "recover", shellQuote(source), ...sourceFlags(partition, volume), ...references, "--output", destination ? shellQuote(destination) : "<destination>"].join(" ");
}

/** Copies text to the clipboard; resolves to whether it worked. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
