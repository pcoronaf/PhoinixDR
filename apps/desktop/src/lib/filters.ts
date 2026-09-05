import type { CandidateSummary, HealthCategory } from "../types";

/** Categories from best to worst. */
export const CATEGORY_ORDER: HealthCategory[] = [
  "Excellent",
  "VeryGood",
  "Good",
  "Poor",
  "VeryPoor",
  "Unrecoverable",
  "Unknown",
];

export const CATEGORY_LABEL: Record<HealthCategory, string> = {
  Excellent: "Excellent",
  VeryGood: "Very good",
  Good: "Good",
  Poor: "Poor",
  VeryPoor: "Very poor",
  Unrecoverable: "Unrecoverable",
  Unknown: "Unknown",
};

/** Rank of a category: lower is better. */
export function categoryRank(c: HealthCategory): number {
  const i = CATEGORY_ORDER.indexOf(c);
  return i < 0 ? CATEGORY_ORDER.length : i;
}

export type SourceFilter = "all" | "metadata" | "carved";

export interface Filters {
  search: string;
  minCategory: HealthCategory | null;
  source: SourceFilter;
  /** Type ids to keep; empty keeps every type. */
  types: string[];
  /** Folder prefix (as shown in the tree), or null for everything. */
  folder: string | null;
}

export const DEFAULT_FILTERS: Filters = {
  search: "",
  minCategory: null,
  source: "all",
  types: [],
  folder: null,
};

/** Path components of a candidate's original path (without the file name). */
export function folderOf(row: CandidateSummary): string[] {
  if (!row.path) return row.source === "file_carving" ? ["(carved)"] : ["(unknown)"];
  const parts = row.path.split(/[\\/]+/).filter((p) => p.length > 0);
  parts.pop();
  return parts;
}

/** The folder key used by the tree and the filter. */
export function folderKey(parts: string[]): string {
  return parts.join("\\");
}

/** Applies the filters, keeping the input order. */
export function applyFilters(rows: CandidateSummary[], f: Filters): CandidateSummary[] {
  const needle = f.search.trim().toLowerCase();
  const minRank = f.minCategory ? categoryRank(f.minCategory) : Number.POSITIVE_INFINITY;
  return rows.filter((r) => {
    if (needle && !r.name.toLowerCase().includes(needle) && !(r.path ?? "").toLowerCase().includes(needle)) {
      return false;
    }
    if (f.minCategory && (r.category === "Unknown" || categoryRank(r.category) > minRank)) return false;
    if (f.source === "metadata" && r.source === "file_carving") return false;
    if (f.source === "carved" && r.source !== "file_carving") return false;
    if (f.types.length > 0 && !f.types.includes(r.type_id ?? "(none)")) return false;
    if (f.folder !== null) {
      const key = folderKey(folderOf(r));
      if (key !== f.folder && !key.startsWith(f.folder + "\\")) return false;
    }
    return true;
  });
}

export interface TypeOption {
  id: string;
  name: string;
  count: number;
}

/** The types present in `rows`, most frequent first. */
export function typeOptions(rows: CandidateSummary[]): TypeOption[] {
  const map = new Map<string, TypeOption>();
  for (const r of rows) {
    const id = r.type_id ?? "(none)";
    const entry = map.get(id) ?? { id, name: r.type_name ?? "Unknown type", count: 0 };
    entry.count += 1;
    map.set(id, entry);
  }
  return [...map.values()].sort((a, b) => b.count - a.count || a.id.localeCompare(b.id));
}

export interface TreeNode {
  name: string;
  key: string;
  count: number;
  children: TreeNode[];
}

/** Builds the folder tree of `rows`; counts include descendants. */
export function buildTree(rows: CandidateSummary[]): TreeNode {
  const root: TreeNode = { name: "All files", key: "", count: rows.length, children: [] };
  for (const r of rows) {
    let node = root;
    const parts = folderOf(r);
    for (let i = 0; i < parts.length; i += 1) {
      const name = parts[i] ?? "";
      const key = folderKey(parts.slice(0, i + 1));
      let child = node.children.find((c) => c.key === key);
      if (!child) {
        child = { name, key, count: 0, children: [] };
        node.children.push(child);
        node.children.sort((a, b) => a.name.localeCompare(b.name));
      }
      child.count += 1;
      node = child;
    }
  }
  return root;
}

/** Sort rows by a column. */
export type SortKey = "name" | "size" | "likelihood" | "modified" | "path";

export function sortRows(rows: CandidateSummary[], key: SortKey, ascending: boolean): CandidateSummary[] {
  const dir = ascending ? 1 : -1;
  const cmp = (a: CandidateSummary, b: CandidateSummary): number => {
    switch (key) {
      case "name":
        return a.name.localeCompare(b.name);
      case "size":
        return (a.size ?? -1) - (b.size ?? -1);
      case "likelihood":
        return a.likelihood - b.likelihood || a.confidence - b.confidence;
      case "modified":
        return (a.modified ?? "").localeCompare(b.modified ?? "");
      case "path":
        return (a.path ?? "").localeCompare(b.path ?? "");
      default:
        return 0;
    }
  };
  return [...rows].sort((a, b) => dir * cmp(a, b));
}
