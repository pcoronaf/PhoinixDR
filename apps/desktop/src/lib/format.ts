const UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/** Bytes in IEC units with one decimal above KiB. */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined) return "–";
  if (bytes < 1024) return `${bytes} B`;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${UNITS[unit]}`;
}

/** Bytes in SI units (device sizes). */
export function formatBytesSi(bytes: number): string {
  const units = ["B", "kB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

/** An ISO timestamp as a local date/time, or a dash. */
export function formatDate(iso: string | null | undefined): string {
  if (!iso) return "–";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

/** Unix seconds as a local date/time. */
export function formatUnix(seconds: number): string {
  return new Date(seconds * 1000).toLocaleString();
}

/** A percentage label. */
export function percent(done: number, total: number | null): string {
  if (!total) return "";
  return `${Math.min(100, Math.floor((done / total) * 100))}%`;
}
