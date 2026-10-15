// Sample data for running the front-end in a plain browser (`npm run dev`
// outside Tauri). Nothing here reaches production paths inside the app.
import type {
  AppInfo,
  CandidateSummary,
  DeviceInfo,
  EngineLogLine,
  EraseCatalogue,
  EraseEvent,
  EraseRequest,
  EraseSummary,
  EraseTarget,
  EraseTargetInfo,
  FileEntry,
  MountedVolumeInfo,
  PartitionCandidate,
  Preview,
  RecoverItem,
  RecoveryCandidate,
  ScanEvent,
  ScanRequest,
  SessionSummary,
  SourceInfo,
  TargetDescription,
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

// ---------------------------------------------------------------------------
// Secure erase demo

export const demoEraseCatalogue: EraseCatalogue = {
  methods: [
    { id: "zero", label: "Zeros, one pass", note: "NIST 800-88 Clear. The fastest method and the default for magnetic disks; one pass is enough on any drive made this century.", passes: 1, multi_pass: false },
    { id: "random", label: "Random data, one pass", note: "NIST 800-88 Clear. Leaves no readable pattern, so an erased disk cannot be told from an encrypted one.", passes: 1, multi_pass: false },
    { id: "nist", label: "NIST SP 800-88 Clear (random, verified)", note: "The documented Clear procedure: one random pass followed by the read-back verification SP 800-88 asks for.", passes: 1, multi_pass: false },
    { id: "dod-3", label: "DoD 5220.22-M (E), 3 passes", note: "The DoD 5220.22-M three-pass sequence (0x00, 0xFF, random). Removed from the NISPOM in 2007; kept because policies still name it.", passes: 3, multi_pass: true },
    { id: "dod-7", label: "DoD 5220.22-M (ECE), 7 passes", note: "The DoD 5220.22-M ECE seven-pass sequence. Obsolete since 2007; kept because policies still name it.", passes: 7, multi_pass: true },
    { id: "schneier", label: "Schneier, 7 passes", note: "0xFF, 0x00 and five random passes, from Applied Cryptography (1996).", passes: 7, multi_pass: true },
    { id: "vsitr", label: "VSITR (BSI), 7 passes", note: "The German BSI 1999 sequence: alternating 0x00/0xFF three times, then 0xAA.", passes: 7, multi_pass: true },
    { id: "gutmann", label: "Gutmann, 35 passes", note: "Peter Gutmann's 1996 sequence for MFM/RLL encodings. The author's epilogue: \u201cperforming the full 35-pass overwrite is pointless for any drive\u201d.", passes: 35, multi_pass: true },
  ],
  warning: "Erasing is irreversible. PhoinixDR will overwrite the target you confirm; nothing on it can be recovered afterwards, by PhoinixDR or by anyone. Check the model, serial number and size before confirming, and unplug every disk you do not intend to erase.",
  multi_pass_warning: "More than one pass adds time and wear, not security. Multi-pass schemes come from 1990s drive encodings; Gutmann's own epilogue calls the full 35-pass method \u201cpointless for any drive\u201d, and NIST SP 800-88 states that one overwrite protects drives made after 2001 against both keyboard and laboratory attack. The method is offered because some policies still require it.",
  flash_warning: "This is a solid-state device. Overwriting reaches the logical blocks, not the physical cells: the controller remaps writes, and over-provisioned or retired cells are never touched. An overwrite is a software-level clear (NIST 800-88 Clear); a purge needs the device's own sanitize command (ATA Secure Erase, NVMe Sanitize) or physical destruction.",
  article_gutmann: "https://www.pcoronaf.com/2014/07/el-mito-de-gutmann.html",
  article_files: "https://www.pcoronaf.com/2020/12/borrado-seguro-de-archivos.html",
  free_space_note: "A free-space wipe overwrites every cluster the filesystem marks as free, and the slack at the end of each file when asked. It does not reach the content of small deleted files kept inside NTFS MFT records or ext inodes, copies in the journal, volume shadow copies or previous versions, nor the physical cells a solid-state drive has remapped. Deleted names, sizes and dates remain visible in the metadata.",
  files_note: "Erasing files overwrites their current content through the filesystem (every named stream on NTFS, and the slack up to the end of the last cluster), then truncates, renames and deletes them. It cannot reach the copies the system keeps elsewhere: earlier versions in the journal, volume shadow copies and previous versions, temporary files, the swap or page file, backups and cloud copies, the old resident bytes an NTFS record may keep until it is reused, nor the physical cells a solid-state drive has remapped. The deleted name may survive in directory metadata. Follow with a free-space wipe of the volume to cover what the filesystem left behind.",
  mounted_note: "Writing through the filesystem fills the space the volume reports as free; the filesystem keeps some clusters back, and small deleted files kept inside NTFS MFT records or ext inodes, journal copies, shadow copies, previous versions and remapped flash cells are not reached. The volume stays in use while it fills; it is full for a moment, then the fill files are removed.",
};

export const demoEraseVolumes: () => MountedVolumeInfo[] = () => [
  { mount: "C:\\", node: "\\\\.\\C:", filesystem: "NTFS", label: "Windows", total: 511_000_000_000, free: 212_400_000_000, disk: "\\\\.\\PhysicalDrive0", read_only: false, system: true },
  { mount: "D:\\", node: "\\\\.\\D:", filesystem: "NTFS", label: "Data", total: 1_000_000_000_000, free: 388_000_000_000, disk: "\\\\.\\PhysicalDrive1", read_only: false, system: false },
  { mount: "E:\\", node: "\\\\.\\E:", filesystem: "FAT32", label: "STICK", total: 15_500_000_000, free: 9_800_000_000, disk: "\\\\.\\PhysicalDrive2", read_only: false, system: false },
];

function demoDescription(device: DeviceInfo, target: EraseTarget): TargetDescription {
  const partitions = [{ index: 1, offset: 1_048_576, length: device.size - 1_048_576, type_name: device.removable ? "FAT32 (LBA)" : "Microsoft basic data" }];
  const wanted = target.kind === "partition" ? target.index : target.kind === "free_space" ? (target.partition ?? 1) : null;
  const part = wanted !== null ? partitions.find((p) => p.index === wanted) ?? null : null;
  const refusals: TargetDescription["refusals"] = [];
  if (!device.removable) refusals.push({ kind: "system_disk" });
  if (wanted !== null && !part) refusals.push({ kind: "no_such_partition", index: wanted });
  const free = part ? Math.round(part.length * 0.63) : 0;
  const slackBytes = target.kind === "free_space" && target.slack ? 4_318_208 : 0;
  const space: TargetDescription["space"] =
    target.kind === "free_space" && part
      ? { filesystem: device.removable ? "FAT32" : "NTFS", volume_offset: part.offset, volume_length: part.length, cluster_size: 4096, map_available: true, free_bytes: free, free_ranges: 1_204, slack: target.slack, slack_bytes: slackBytes, slack_ranges: target.slack ? 2_113 : 0, mount: null, node: null, label: null, total_bytes: null }
      : null;
  return {
    target,
    path: device.path,
    disk: device.path,
    device,
    offset: part ? part.offset : 0,
    length: space ? space.free_bytes + space.slack_bytes : part ? part.length : device.size,
    sector_size: 512,
    partition: part,
    partitions,
    volumes: device.removable ? [{ node: "\\\\?\\Volume{demo}", mount: "E:\\", can_dismount: true }] : [{ node: "\\\\?\\Volume{sys}", mount: "C:\\", can_dismount: true }],
    solid_state: device.rotational === false,
    refusals,
    confirmation: device.serial ?? device.path,
    confirmation_short: device.serial && device.serial.length > 6 ? device.serial.slice(-6) : null,
    ranges: [],
    space,
    warnings: space ? [demoEraseCatalogue.free_space_note] : [],
    files: null,
  };
}

function demoFilesDescription(target: Extract<EraseTarget, { kind: "files" }>): TargetDescription {
  const v = demoEraseVolumes()[1]!;
  const device = demoDevices.find((d) => d.path === v.disk) ?? null;
  const folder = target.paths.length === 1 && !/\.[a-z0-9]{2,4}$/i.test(target.paths[0]!);
  const entries: FileEntry[] = folder
    ? [
        { path: `${target.paths[0]}\\draft.docx`, size: 48_213, streams: [{ name: "Zone.Identifier", size: 26 }], read_only: false },
        { path: `${target.paths[0]}\\scan.pdf`, size: 2_310_442, streams: [], read_only: true },
        { path: `${target.paths[0]}\\notes\\interview.txt`, size: 1_902, streams: [], read_only: false },
        { path: `${target.paths[0]}\\notes\\photos\\IMG_0412.jpg`, size: 3_188_730, streams: [], read_only: false },
      ]
    : target.paths.map((p, i) => ({ path: p, size: [48_213, 2_310_442, 1_902][i % 3]!, streams: i === 0 ? [{ name: "Zone.Identifier", size: 26 }] : [], read_only: false }));
  const unit = 4096;
  const padded = (n: number) => (n === 0 ? 0 : Math.ceil(n / unit) * unit);
  const perPass = entries.reduce((a, e) => a + padded(e.size) + e.streams.reduce((b, s) => b + padded(s.size), 0), 0);
  return {
    target,
    path: target.paths[0]!,
    disk: v.disk,
    device,
    offset: 0,
    length: perPass,
    sector_size: 1,
    partition: null,
    partitions: [],
    volumes: [],
    solid_state: device ? device.rotational === false : null,
    refusals: [],
    confirmation: target.paths.length === 1 ? target.paths[0]!.split(/[\\/]/).filter((x) => x.length > 0).pop() ?? "root" : `${target.paths.length} items`,
    confirmation_short: null,
    ranges: [],
    space: { filesystem: v.filesystem, volume_offset: 0, volume_length: v.total, cluster_size: unit, map_available: true, free_bytes: v.free, free_ranges: 0, slack: true, slack_bytes: 0, slack_ranges: 0, mount: v.mount, node: v.node, label: v.label, total_bytes: v.total },
    warnings: [demoEraseCatalogue.files_note],
    files: {
      entries,
      directories: folder ? [`${target.paths[0]}\\notes\\photos`, `${target.paths[0]}\\notes`, target.paths[0]!] : [],
      total_bytes: entries.reduce((a, e) => a + e.size + e.streams.reduce((b, s) => b + s.size, 0), 0),
      stream_count: entries.reduce((a, e) => a + e.streams.length, 0),
      allocation_unit: unit,
      skipped: [],
      renamed: [],
    },
  };
}

function demoMountedDescription(target: Extract<EraseTarget, { kind: "mounted_free_space" }>): TargetDescription {
  const v = demoEraseVolumes().find((x) => x.mount === target.mount) ?? demoEraseVolumes()[2]!;
  const device = demoDevices.find((d) => d.path === v.disk) ?? null;
  const free = Math.max(0, v.free - target.reserve);
  const warnings = [demoEraseCatalogue.mounted_note];
  if (v.system) warnings.push("This volume belongs to the system disk. While it is full the system and other programs may misbehave; close them first, and let the wipe finish (the files are removed at the end).");
  return {
    target,
    path: v.mount,
    disk: v.disk,
    device,
    offset: 0,
    length: free,
    sector_size: 1,
    partition: null,
    partitions: [],
    volumes: [],
    solid_state: device ? device.rotational === false : null,
    refusals: v.read_only ? [{ kind: "unsupported", reason: `${v.mount} is mounted read-only` }] : [],
    confirmation: v.label ?? v.mount.replace(/[\\/]+$/, ""),
    confirmation_short: null,
    ranges: [],
    space: { filesystem: v.filesystem, volume_offset: 0, volume_length: v.total, cluster_size: 0, map_available: true, free_bytes: free, free_ranges: 0, slack: false, slack_bytes: 0, slack_ranges: 0, mount: v.mount, node: v.node, label: v.label, total_bytes: v.total },
    warnings,
    files: null,
  };
}

export function demoEraseTargets(): EraseTargetInfo[] {
  return demoDevices.map((device) => {
    const d = { ...device, accessible: true, serial: device.serial === "?8" ? "0781C3A9D2F1" : device.serial };
    const description = demoDescription(d, { kind: "whole_device", path: d.path });
    return { device: d, description, refusals: description.refusals, error: null };
  });
}

export function demoDescribe(target: EraseTarget): TargetDescription {
  if (target.kind === "mounted_free_space") return demoMountedDescription(target);
  if (target.kind === "files") return demoFilesDescription(target);
  const device = demoEraseTargets().find((t) => t.device.path === target.path)?.device ?? demoEraseTargets()[1]!.device;
  return demoDescription(device, target);
}

export function demoErase(request: EraseRequest, emit: (e: EraseEvent) => void, cancelled: () => boolean): void {
  const description = demoDescribe(request.target);
  const method = demoEraseCatalogue.methods.find((m) => m.id === request.method.id) ?? demoEraseCatalogue.methods[0]!;
  const passes = request.method.id === "custom" && "passes" in request.method ? request.method.passes.length : method.passes;
  const labels = ["0x00", "0xFF", "random"];
  const total = description.length;
  const scanned = request.post_scan !== "none";
  const scanText = request.post_scan === "deep" ? "post-erase deep scan found nothing recoverable" : "post-erase quick scan found nothing recoverable";
  const summary = (done: number, verified: boolean): EraseSummary => ({
    passes_completed: done,
    passes,
    bytes_written: done * total,
    unwritable_bytes: 0,
    unwritable_ranges: 0,
    seconds: done * 3,
    verification: verified ? { mode: request.verify, blocks_checked: 4096, blocks_mismatched: 0, bytes_checked: 4096 * 65536, skipped_unwritable_bytes: 0, first_mismatch: null, cancelled: false, passed: true } : null,
    files_erased: verified && description.files ? description.files.entries.length : 0,
    files_failed: [],
    scan: verified && scanned ? { mode: request.post_scan, ran: true, note: null, filesystem: description.space?.filesystem ?? null, partition_table: "MBR", candidates: 1_312, with_content: 0, carved: 0, bytes_scanned: request.post_scan === "deep" ? total : 0, seconds: 41, cancelled: false, passed: true } : null,
    certificate: request.certificate,
    verdict: verified ? `${passes} pass(es) completed; verified: 4096 blocks read back, all match${scanned ? `; ${scanText}` : ""}${description.files ? `; ${description.files.entries.length} file(s) erased` : ""}` : `cancelled after ${done} of ${passes} pass(es): the target is PARTIALLY erased`,
  });
  emit({ kind: "started", description, method: method.label, passes, resumed_from: null });
  let pass = 1;
  let done = 0;
  const step = (): void => {
    if (cancelled()) {
      emit({ kind: "cancelled", resume: request.target.kind === "mounted_free_space" || request.target.kind === "files" ? null : { key: "demo", method: method.id, pass: pass - 1, offset: done, seed: "demo", bytes_written: (pass - 1) * total + done }, summary: summary(pass - 1, false) });
      return;
    }
    if (done === 0) emit({ kind: "pass", pass, passes, pattern: labels[(pass - 1) % labels.length]! });
    done = Math.min(total, done + total / 12);
    emit({ kind: "progress", pass, passes, done, total, bytes_per_sec: 180_000_000, unwritable: 0 });
    if (done >= total) {
      if (pass >= passes) {
        let v = 0;
        const finish = (): void => {
          if (!scanned) {
            emit({ kind: "finished", summary: summary(passes, true) });
            return;
          }
          let n = 0;
          const phases = request.post_scan === "deep" ? ["metadata", "carving", "assembling"] : ["metadata"];
          const scan = (): void => {
            n += 1;
            const phase = phases[Math.min(phases.length - 1, Math.floor((n - 1) / 6))]!;
            emit({ kind: "scanning", mode: request.post_scan, phase, done: (n % 6) * 1_000_000_000, total: 6_000_000_000, candidates: Math.min(1_312, n * 90) });
            if (n < phases.length * 6) window.setTimeout(scan, 150);
            else emit({ kind: "finished", summary: summary(passes, true) });
          };
          window.setTimeout(scan, 100);
        };
        const verify = (): void => {
          v += 512;
          emit({ kind: "verifying", done: Math.min(v, 4096), total: 4096 });
          if (v < 4096) window.setTimeout(verify, 120);
          else finish();
        };
        window.setTimeout(verify, 200);
        return;
      }
      pass += 1;
      done = 0;
    }
    window.setTimeout(step, 180);
  };
  window.setTimeout(step, 300);
}
