// DTOs mirroring crates/phoinix-session/src/dto.rs (JSON shapes are the contract).

// Serialised in kebab-case by phoinix-core.
export type FileSystemType =
  | "ntfs" | "fat12" | "fat16" | "fat32" | "ex-fat" | "ext" | "hfs" | "hfs-plus" | "apfs" | "unknown";

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

export interface Repair {
  offset: number;
  bytes: number[];
  description: string;
}

export interface VolumeInfo {
  partition: number | null;
  offset: number;
  length: number;
  type_description: string;
  filesystem: FileSystemType;
  confidence: number;
  supported: boolean;
  lost?: boolean;
  repairs?: Repair[];
}

export interface VolumeRange {
  offset: number;
  length: number;
  repairs: Repair[];
}

export type FoundVia = "primary_boot_sector" | "backup_boot_sector" | "superblock" | "backup_superblock";

export type PartitionRelation =
  | { kind: "listed"; index: number }
  | { kind: "lost" }
  | { kind: "inside_partition"; index: number }
  | { kind: "nested"; within: number }
  | { kind: "overlapping"; with: number };

export interface PartitionCandidate {
  start: number;
  length: number;
  readable_length: number;
  filesystem: FileSystemType;
  label: string | null;
  serial: string | null;
  cluster_size: number | null;
  sector_size: number;
  found_via: FoundVia;
  primary_structure_valid: boolean;
  backup_structure_valid: boolean | null;
  geometry_consistent: boolean;
  engine_verified: boolean | null;
  root_entries: number | null;
  relation: PartitionRelation;
  repairs: Repair[];
  evidence: { supports: boolean; description: string }[];
  confidence: number;
}

export type SearchEvent =
  | { kind: "progress"; done: number; total: number }
  | { kind: "finished"; candidates: PartitionCandidate[] }
  | { kind: "failed"; message: string };

export type ImageFormat = "raw" | "split-raw" | "ewf" | "vhd" | "vhdx" | "vmdk";

export interface StoredHashes {
  md5: string | null;
  sha1: string | null;
}

export interface AcquisitionInfo {
  case_number: string | null;
  evidence_number: string | null;
  description: string | null;
  examiner: string | null;
  notes: string | null;
  acquisition_date: string | null;
  system_date: string | null;
  software_version: string | null;
  operating_system: string | null;
  model: string | null;
  serial_number: string | null;
}

export interface ContainerInfo {
  format: ImageFormat;
  variant: string;
  segments: string[];
  size: number;
  sector_size: number;
  unit_size: number | null;
  compression: string | null;
  identifier: string | null;
  media_type: string | null;
  stored_hashes: StoredHashes;
  acquisition: AcquisitionInfo | null;
  acquisition_errors: number | null;
  diagnostics: string[];
}

export interface HashVerification {
  bytes: number;
  md5: string;
  sha1: string;
  sha256: string;
  stored: StoredHashes;
  md5_matches: boolean | null;
  sha1_matches: boolean | null;
}

export interface VerifyEvent {
  done: number;
  total: number;
}

export interface CaseMetadata {
  case_number: string | null;
  evidence_number: string | null;
  examiner: string | null;
  notes: string | null;
}

export interface SourceInfo {
  path: string;
  is_device: boolean;
  size: number;
  sector_size: number;
  scheme: string;
  volumes: VolumeInfo[];
  container?: ContainerInfo | null;
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
  volume?: VolumeRange | null;
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
  case?: CaseMetadata | null;
  report?: string | null;
  verify_source?: boolean;
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
  | { kind: "finished"; items: RecoverItem[]; failures: number; report?: string | null }
  | { kind: "verifying"; done: number; total: number };

export interface AppInfo {
  version: string;
  author?: string;
  sessions_dir: string;
  device_access: boolean;
}
