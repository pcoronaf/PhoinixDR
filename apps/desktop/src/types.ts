// DTOs mirroring crates/phoinix-session/src/dto.rs (JSON shapes are the contract).

export type FileSystemType =
  | "Ntfs" | "Fat12" | "Fat16" | "Fat32" | "ExFat" | "Ext" | "Hfs" | "HfsPlus" | "Apfs" | "Unknown";

export type HealthCategory =
  | "Unrecoverable" | "VeryPoor" | "Poor" | "Good" | "VeryGood" | "Excellent" | "Unknown";

export type CandidateSource =
  | "filesystem_metadata" | "journal" | "file_carving" | "partition_reconstruction" | "snapshot" | "combined";

export interface DeviceInfo {
  id: string;
  path: string;
  display_name: string;
  kind: string;
  parent: string | null;
  size: number;
  geometry: { logical_sector_size: number; physical_sector_size: number | null; alignment: number | null };
  removable: boolean | null;
  rotational: boolean | null;
  bus: string | null;
  vendor: string | null;
  model: string | null;
  serial: string | null;
  accessible: boolean;
}

export interface VolumeInfo {
  partition: number | null;
  offset: number;
  length: number;
  type_description: string;
  filesystem: FileSystemType;
  confidence: number;
  supported: boolean;
}

export interface SourceInfo {
  path: string;
  is_device: boolean;
  size: number;
  sector_size: number;
  scheme: string;
  volumes: VolumeInfo[];
  diagnostics: string[];
}

export type ScanMode = "quick" | "deep";

export interface CarveSettings {
  whole_volume: boolean;
  types: string[];
  min_size: number;
  alignment: number;
}

export interface ScanRequest {
  source: string;
  partition: number | null;
  mode: ScanMode;
  examine_content: boolean;
  carve: CarveSettings;
}

export type ScanPhase = "opening" | "metadata" | "carving" | "finishing";

export interface CandidateSummary {
  id: string;
  name: string;
  path: string | null;
  path_uncertain: boolean;
  size: number | null;
  category: HealthCategory;
  likelihood: number;
  confidence: number;
  source: CandidateSource;
  type_id: string | null;
  type_name: string | null;
  modified: string | null;
  reference: string;
}

export interface CarveReport {
  bytes_scanned: number;
  bytes_eligible: number;
  hits: number;
  nested_skipped: number;
  rejected: number;
  too_small: number;
  candidates: number;
  merged_into_metadata: number;
}

export interface SessionSummary {
  id: string;
  file: string | null;
  source: string;
  partition: number | null;
  filesystem: FileSystemType;
  mode: ScanMode;
  started: number;
  finished: number | null;
  complete: boolean;
  candidates: number;
  from_metadata: number;
  carved: number;
  carving: CarveReport | null;
}

export type ScanEvent =
  | { kind: "started"; session_id: string; filesystem: FileSystemType; volume: VolumeInfo }
  | { kind: "phase"; phase: ScanPhase }
  | { kind: "progress"; phase: ScanPhase; done: number; total: number | null; candidates: number }
  | { kind: "candidates"; items: CandidateSummary[] }
  | { kind: "finished"; summary: SessionSummary }
  | { kind: "failed"; message: string }
  | { kind: "cancelled"; summary: SessionSummary };

export type ScanCompletion =
  | { kind: "session"; summary: SessionSummary; cancelled: boolean }
  | { kind: "failed"; message: string };

export interface HealthReason {
  positive: boolean;
  text: string;
}

export interface ValidationCheck {
  name: string;
  passed: boolean;
  detail: string;
}

export interface FileTypeDetection {
  id: string;
  name: string;
  extension: string;
}

// The full candidate as serialised by phoinix-fs (only the fields the UI reads are typed).
export interface RecoveryCandidate {
  id: string;
  filesystem: FileSystemType;
  filesystem_object: Record<string, unknown> & { filesystem: string };
  original_name: string | null;
  original_path: string | null;
  path_uncertain: boolean;
  logical_size: number | null;
  deleted: boolean;
  timestamps: {
    created_iso: string | null;
    modified_iso: string | null;
    accessed_iso: string | null;
  };
  evidence: {
    source: CandidateSource;
    metadata: Record<string, unknown>;
    extents: {
      resident: boolean;
      complete: boolean;
      extent_count: number;
      total_clusters: number | null;
      expected_clusters: number | null;
      chain_known: boolean;
      heuristic: boolean;
      start_inferred: boolean;
    };
    allocation: {
      clusters_total: number;
      clusters_free: number;
      clusters_allocated: number;
      clusters_unknown: number;
      map_available: boolean;
    };
    content: {
      detected_type: FileTypeDetection | null;
      expected_type: FileTypeDetection | null;
      validation: { status: string; checks: ValidationCheck[] } | null;
      zero_block_ratio: number | null;
      bytes_examined: number;
    };
    storage: { device_kind: string; rotational: boolean | null };
    diagnostics: { severity: "info" | "warning"; message: string }[];
  };
  health: {
    likelihood: number;
    confidence: number;
    category: HealthCategory;
    reasons: HealthReason[];
  };
}

export type Preview =
  | { kind: "image"; mime: string; base64: string; bytes: number }
  | { kind: "text"; text: string; truncated: boolean }
  | { kind: "hex"; dump: string; bytes: number }
  | { kind: "unavailable"; reason: string };

export interface DestinationInfo {
  destination: string;
  same_disk: boolean | null;
  overwrites_source_image: boolean;
  dangerous: boolean;
  warning: string | null;
}

export interface RecoverRequest {
  candidates: string[];
  destination: string;
  preserve_tree: boolean;
  preserve_timestamps: boolean;
  hash: boolean;
  overwrite: boolean;
  allow_same_device: boolean;
}

export interface RecoveryResult {
  output_path: string;
  bytes_expected: number | null;
  bytes_written: number;
  sha256: string | null;
  complete: boolean;
  diagnostics: { message: string }[];
}

export interface RecoverItem {
  id: string;
  name: string;
  result: RecoveryResult | null;
  error: string | null;
}

export type RecoverEvent =
  | { kind: "started"; total: number; warning: string | null }
  | { kind: "item"; index: number; total: number; item: RecoverItem }
  | { kind: "finished"; items: RecoverItem[]; failures: number };

export interface AppInfo {
  version: string;
  sessions_dir: string;
  device_access: boolean;
}
