// Sample data for running the front-end in a plain browser (`npm run dev`
// outside Tauri). Nothing here reaches production paths inside the app.
import type {
  AppInfo,
  CandidateSummary,
  DeviceInfo,
  PartitionCandidate,
  Preview,
  RecoverItem,
  RecoveryCandidate,
  ScanEvent,
  ScanRequest,
  SessionSummary,
  SourceInfo,
  EngineLogLine,
} from "./types";

export const demoDevices: DeviceInfo[] = [
  {
    id: "d1",
    path: "\\\\.\\PhysicalDrive0",
    display_name: "ADATA SU800NS38",
    kind: "disk",
    parent: null,
    size: 512_110_190_592,
    geometry: { logical_sector_size: 512, physical_sector_size: 512, alignment: null },
    removable: false,
    rotational: false,
    bus: "sata",
    vendor: null,
    model: "ADATA SU800NS38",
    serial: "2K27",
    accessible: false, // the demo mimics a session without administrator rights
  },
  {
    id: "d2",
    path: "\\\\.\\PhysicalDrive1",
    display_name: "Generic Flash Disk",
    kind: "disk",
    parent: null,
    size: 16_357_785_600,
    geometry: { logical_sector_size: 512, physical_sector_size: null, alignment: null },
    removable: true,
    rotational: null,
    bus: "usb",
    vendor: "Generic",
    model: "Flash Disk",
    serial: "?8",
    accessible: true,
  },
];

export function demoSource(path: string): SourceInfo {
  return {
    path,
    is_device: path.startsWith("\\\\.\\"),
    size: 16_357_785_600,
    sector_size: 512,
    scheme: "Mbr",
    volumes: [
      { partition: 1, offset: 1_048_576, length: 16_356_737_024, type_description: "FAT32 (LBA)", filesystem: "fat32", confidence: 92, supported: true },
    ],
    diagnostics: [],
  };
}

const names: [string, string, string, number, number, number][] = [
  ["Presentación Riesgos.pptx", "\\Presentación Riesgos.pptx", "pptx", 185_713_708, 95, 87],
  ["ISACA-J-S-26-00003.pdf", "\\ISACA-J-S-26-00003.pdf", "pdf", 934_224, 79, 67],
  ["composed.jpg", "\\photos\\composed.jpg", "jpeg", 363_520, 92, 72],
  ["00-COMPLETE-BOOK.zip", "\\00-COMPLETE-BOOK.zip", "zip", 300_032, 59, 52],
  ["notes.txt", "\\docs\\notes.txt", "(none)", 9_000, 59, 52],
  ["e1214424-8ad3.bin", "\\SOURCES\\e1214424-8ad3.bin", "(none)", 316_416, 15, 62],
];

export const demoRows: CandidateSummary[] = names.map(([name, path, type, size, likelihood, confidence], i) => ({
  id: `c${i}`,
  name,
  path,
  path_uncertain: false,
  size,
  category: likelihood >= 95 ? "Excellent" : likelihood >= 80 ? "VeryGood" : likelihood >= 60 ? "Good" : likelihood >= 35 ? "Poor" : "VeryPoor",
  likelihood,
  confidence,
  source: "filesystem_metadata",
  type_id: type === "(none)" ? null : type,
  type_name: type === "(none)" ? null : type.toUpperCase(),
  modified: "2026-09-02T16:22:52Z",
  reference: `${16777728 + i * 32}`,
}));

export const demoCarved: CandidateSummary[] = [
  {
    id: "k1",
    name: "carved-000038411776.jpg",
    path: null,
    path_uncertain: true,
    size: 22_219,
    category: "VeryGood",
    likelihood: 85,
    confidence: 72,
    source: "file_carving",
    type_id: "jpeg",
    type_name: "JPEG image",
    modified: null,
    reference: "c38411776",
  },
];

export function demoSession(request: ScanRequest, count: number): SessionSummary {
  return {
    id: "demo-session",
    file: null,
    source: request.source,
    partition: request.partition,
    filesystem: "fat32",
    mode: request.mode,
    started: Math.floor(Date.now() / 1000) - 30,
    finished: Math.floor(Date.now() / 1000),
    complete: true,
    candidates: count,
    from_metadata: demoRows.length,
    carved: count - demoRows.length,
    carving: request.mode === "deep" ? { bytes_scanned: 3_500_000, bytes_eligible: 3_500_000, hits: 3, nested_skipped: 0, rejected: 0, too_small: 0, candidates: 3, merged_into_metadata: 2 } : null,
  };
}

export function demoScan(request: ScanRequest, emit: (e: ScanEvent) => void, log: (line: EngineLogLine) => void = () => {}): void {
  const volume = demoSource(request.source).volumes[0]!;
  const say = (level: EngineLogLine["level"], target: string, message: string): void => log({ time: Date.now(), level, target, message });
  say("info", "phoinix_session::scan", `scan requested source=${request.source} mode=${request.mode === "deep" ? "Deep" : "Quick"} partition=${request.partition ?? "None"} examine_content=${request.examine_content}`);
  say("info", "phoinix_image", `opening image path=${request.source} format=raw`);
  say("info", "phoinix_block::raw", `opened RAW image path=${request.source} length=16357785600`);
  say("debug", "phoinix_volume::mbr", "MBR read partitions=1");
  say("info", "phoinix_fs_fat::volume", "FAT volume opened variant=FAT32 clusters=3993600 cluster_size=4096");
  say("info", "phoinix_session::scan", "volume opened filesystem=FAT32 offset=1048576 length=16356737024 engine=true");
  say("info", "phoinix_session::scan", "metadata scan: walking deleted records");
  emit({ kind: "phase", phase: "opening" });
  emit({ kind: "started", session_id: "demo-session", filesystem: "fat32", volume });
  emit({ kind: "phase", phase: "metadata" });
  let i = 0;
  const tick = (): void => {
    if (i < demoRows.length) {
      say("debug", "phoinix_fs_fat::undelete", `deleted entry cluster=${2048 + i * 37} name=${demoRows[i]!.name}`);
      emit({ kind: "candidates", items: [demoRows[i]!] });
      emit({ kind: "progress", phase: "metadata", done: i + 1, total: null, candidates: i + 1 });
      i += 1;
      window.setTimeout(tick, 150);
      return;
    }
    say("info", "phoinix_session::scan", `metadata scan finished records=${demoRows.length} candidates=${demoRows.length}`);
    if (request.mode === "deep") {
      say("info", "phoinix_session::scan", "carving unallocated space by signature whole_volume=false min_size=0 alignment=512");
      emit({ kind: "phase", phase: "carving" });
      let done = 0;
      const step = (): void => {
        done += 700_000;
        emit({ kind: "progress", phase: "carving", done: Math.min(done, 3_500_000), total: 3_500_000, candidates: demoRows.length + 3 });
        if (done < 3_500_000) window.setTimeout(step, 120);
        else {
          say("info", "phoinix_carve::engine", "header search complete; assembling hits hits=3 ranges=1 eligible=3500000");
          emit({ kind: "phase", phase: "assembling" });
          let examined = 0;
          const assemble = (): void => {
            examined += 1;
            emit({ kind: "progress", phase: "assembling", done: examined, total: 3, candidates: demoRows.length + examined });
            if (examined < 3) {
              window.setTimeout(assemble, 250);
              return;
            }
            finish();
          };
          window.setTimeout(assemble, 250);
        }
      };
      const finish = (): void => {
        {
          say("info", "phoinix_carve::engine", "assembly finished candidates=3 rejected=0 nested_skipped=0 too_small=0 cancelled=false");
          say("info", "phoinix_session::scan", "carving finished carved=3 merged_into_metadata=2");
          emit({ kind: "phase", phase: "finishing" });
          emit({ kind: "candidates", items: demoCarved });
          say("info", "phoinix_session::scan", `scan finished candidates=${demoRows.length + demoCarved.length}`);
          emit({ kind: "finished", summary: demoSession(request, demoRows.length + demoCarved.length) });
        }
      };
      window.setTimeout(step, 120);
      return;
    }
    say("info", "phoinix_session::scan", `scan finished candidates=${demoRows.length}`);
    emit({ kind: "finished", summary: demoSession(request, demoRows.length) });
  };
  window.setTimeout(tick, 200);
}

export function demoDetail(row: CandidateSummary): RecoveryCandidate {
  return {
    id: row.id,
    filesystem: "fat32",
    filesystem_object: { filesystem: "fat", entry_offset: Number(row.reference) || 0 },
    original_name: row.name,
    original_path: row.path,
    path_uncertain: row.path_uncertain,
    logical_size: row.size,
    deleted: true,
    timestamps: { created_iso: null, modified_iso: row.modified, accessed_iso: null },
    evidence: {
      source: row.source,
      metadata: {},
      extents: { resident: false, complete: true, extent_count: 1, total_clusters: 115, expected_clusters: 115, chain_known: false, heuristic: false, start_inferred: true },
      allocation: { clusters_total: 115, clusters_free: 115, clusters_allocated: 0, clusters_unknown: 0, map_available: true },
      content: {
        detected_type: row.type_id ? { id: row.type_id, name: row.type_name ?? row.type_id, extension: row.type_id } : null,
        expected_type: null,
        validation: row.type_id === "pdf" ? { status: "Valid", checks: [{ name: "PDF header", passed: true, detail: "%PDF-1.4" }, { name: "End-of-file marker", passed: true, detail: "%%EOF present" }] } : null,
        zero_block_ratio: 0,
        bytes_examined: row.size ?? 0,
      },
      storage: { device_kind: "BlockDevice", rotational: null },
      diagnostics: [{ severity: "warning", message: "The high word of the first cluster was cleared on deletion and the recorded cluster 8,241 is allocated to other data; cluster 73,777 was chosen among 1 free candidate sharing the low word because its content carries the signature of the type expected from the file name" }],
    },
    health: {
      likelihood: row.likelihood,
      confidence: row.confidence,
      category: row.category,
      reasons: [
        { positive: true, text: "Valid deleted metadata record" },
        { positive: true, text: "Original filename is available" },
        { positive: true, text: "All 115 required clusters are currently free" },
        { positive: false, text: "The recorded start cluster was untrustworthy; the start was inferred from free clusters and their content (heuristic)" },
      ],
    },
  };
}

export function demoPreview(row: CandidateSummary): Preview {
  if (row.type_id === "jpeg") {
    return { kind: "unavailable", reason: "Image previews need the desktop application (demo mode)" };
  }
  return { kind: "text", text: `Demo preview of ${row.name}\n\n(the desktop application shows the reconstructed content here)`, truncated: false };
}

export function demoRecover(ids: string[], rows: CandidateSummary[], destination: string): RecoverItem[] {
  return ids.map((id) => {
    const r = rows.find((x) => x.id === id);
    return {
      id,
      name: r?.name ?? id,
      result: { output_path: `${destination}\\${r?.name ?? id}`, bytes_expected: r?.size ?? null, bytes_written: r?.size ?? 0, sha256: "acdc2332c2c7b929cd6308a831cc91bce777ca05adadeb7f9982b7a250e5ed2a", complete: true, diagnostics: [] },
      error: null,
    };
  });
}

export const demoAppInfo: AppInfo = { version: "0.1.0-demo", author: "@pcoronaf", disclaimer: "PhoinixDR is provided “as is” and is used entirely at your own risk. Data recovery is inherently uncertain, and improper use may result in permanent data loss or damage. Always work from a copy or disk image when possible and recover files to a different storage device.", sessions_dir: "(browser demo)", device_access: true, elevated: false, platform: "windows" };

export function demoPartitions(path: string): PartitionCandidate[] {
  const listed: PartitionCandidate = {
    start: 1_048_576,
    length: 16_356_737_024,
    readable_length: 16_356_737_024,
    filesystem: "fat32",
    label: "STICK",
    serial: "1A2B-3C4D",
    cluster_size: 8192,
    sector_size: 512,
    found_via: "primary_boot_sector",
    primary_structure_valid: true,
    backup_structure_valid: true,
    geometry_consistent: true,
    engine_verified: true,
    root_entries: 14,
    relation: { kind: "listed", index: 1 },
    repairs: [],
    evidence: [{ supports: true, description: `FAT32 boot sector on ${path}` }, { supports: true, description: "the backup boot sector at sector 6 matches" }],
    confidence: 99,
  };
  const lost: PartitionCandidate = {
    ...listed,
    start: 8_589_934_592,
    length: 4_294_967_296,
    readable_length: 4_294_967_296,
    filesystem: "ntfs",
    label: null,
    serial: "3A1F00C9B2E4D511",
    cluster_size: 4096,
    found_via: "backup_boot_sector",
    primary_structure_valid: false,
    root_entries: null,
    relation: { kind: "lost" },
    repairs: [{ offset: 0, bytes: [], description: "backup boot sector substituted for the destroyed primary" }],
    evidence: [{ supports: true, description: "found through the backup boot sector; the primary boot sector is missing or damaged" }, { supports: true, description: "the $MFT lies where the boot sector says" }],
    confidence: 84,
  };
  return [listed, lost];
}
