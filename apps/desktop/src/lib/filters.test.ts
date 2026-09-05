import { describe, expect, it } from "vitest";
import type { CandidateSummary } from "../types";
import { applyFilters, buildTree, DEFAULT_FILTERS, sortRows, typeOptions } from "./filters";
import { formatBytes, fsLabel, hasEngine } from "./format";

function row(over: Partial<CandidateSummary>): CandidateSummary {
  return {
    id: Math.random().toString(36).slice(2),
    name: "file.bin",
    path: "\\docs\\file.bin",
    path_uncertain: false,
    size: 10,
    category: "Good",
    likelihood: 70,
    confidence: 80,
    source: "filesystem_metadata",
    type_id: null,
    type_name: null,
    modified: null,
    reference: "1",
    ...over,
  };
}

const rows: CandidateSummary[] = [
  row({ name: "photo.jpg", path: "\\pictures\\2024\\photo.jpg", type_id: "jpeg", type_name: "JPEG image", category: "Excellent", likelihood: 95, size: 300 }),
  row({ name: "report.pdf", path: "\\docs\\report.pdf", type_id: "pdf", type_name: "PDF document", category: "Poor", likelihood: 40, size: 20 }),
  row({ name: "carved-000000001.png", path: null, type_id: "png", type_name: "PNG image", source: "file_carving", category: "VeryGood", likelihood: 85, size: 5 }),
  row({ name: "notes.txt", path: "\\docs\\notes.txt", category: "Unknown", likelihood: 0, size: null }),
];

describe("applyFilters", () => {
  it("keeps everything by default", () => {
    expect(applyFilters(rows, DEFAULT_FILTERS)).toHaveLength(4);
  });
  it("searches names and paths", () => {
    expect(applyFilters(rows, { ...DEFAULT_FILTERS, search: "DOCS" }).map((r) => r.name)).toEqual(["report.pdf", "notes.txt"]);
  });
  it("applies a minimum category and drops unknowns", () => {
    expect(applyFilters(rows, { ...DEFAULT_FILTERS, minCategory: "Good" }).map((r) => r.name)).toEqual(["photo.jpg", "carved-000000001.png"]);
  });
  it("filters by source and type", () => {
    expect(applyFilters(rows, { ...DEFAULT_FILTERS, source: "carved" })).toHaveLength(1);
    expect(applyFilters(rows, { ...DEFAULT_FILTERS, source: "metadata" })).toHaveLength(3);
    expect(applyFilters(rows, { ...DEFAULT_FILTERS, types: ["pdf", "(none)"] })).toHaveLength(2);
  });
  it("filters by folder including subfolders", () => {
    expect(applyFilters(rows, { ...DEFAULT_FILTERS, folder: "pictures" })).toHaveLength(1);
    expect(applyFilters(rows, { ...DEFAULT_FILTERS, folder: "docs" })).toHaveLength(2);
    expect(applyFilters(rows, { ...DEFAULT_FILTERS, folder: "(carved)" })).toHaveLength(1);
  });
});

describe("tree, types and sorting", () => {
  it("builds a folder tree with counts", () => {
    const tree = buildTree(rows);
    expect(tree.count).toBe(4);
    const docs = tree.children.find((c) => c.name === "docs");
    expect(docs?.count).toBe(2);
    const pictures = tree.children.find((c) => c.name === "pictures");
    expect(pictures?.children[0]?.key).toBe("pictures\\2024");
    expect(tree.children.map((c) => c.name)).toEqual(["(carved)", "docs", "pictures"]);
  });
  it("lists types by frequency", () => {
    const types = typeOptions(rows);
    expect(types.map((t) => t.id)).toEqual(["(none)", "jpeg", "pdf", "png"]);
  });
  it("sorts by likelihood and size", () => {
    expect(sortRows(rows, "likelihood", false).map((r) => r.likelihood)).toEqual([95, 85, 40, 0]);
    expect(sortRows(rows, "size", true).map((r) => r.size)).toEqual([null, 5, 20, 300]);
  });
});

describe("format helpers", () => {
  it("labels filesystem identifiers as serialised by the core", () => {
    expect(fsLabel("ntfs")).toBe("NTFS");
    expect(fsLabel("ex-fat")).toBe("exFAT");
    expect(fsLabel("hfs-plus")).toBe("HFS+");
    expect(fsLabel("something")).toBe("something");
    expect(hasEngine("fat32")).toBe(true);
    expect(hasEngine("ext")).toBe(true);
    expect(hasEngine("unknown")).toBe(false);
  });
  it("formats sizes", () => {
    expect(formatBytes(null)).toBe("–");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KiB");
    expect(formatBytes(185_713_708)).toBe("177 MiB");
  });
});
