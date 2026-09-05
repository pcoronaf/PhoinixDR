// The bridge to the Tauri shell. Every call is a typed command of
// apps/desktop/src-tauri/src/commands.rs; outside Tauri (plain browser) a
// demo implementation keeps the UI usable for development.
import type {
  AppInfo,
  CandidateSummary,
  DestinationInfo,
  DeviceInfo,
  HashVerification,
  PartitionCandidate,
  Preview,
  RecoverEvent,
  RecoverItem,
  RecoverRequest,
  RecoveryCandidate,
  ScanCompletion,
  ScanEvent,
  ScanRequest,
  SearchEvent,
  SessionSummary,
  SourceInfo,
  VerifyEvent,
} from "./types";
import * as demo from "./demo";

export type Unlisten = () => void;

export interface Api {
  isDemo: boolean;
  appInfo(): Promise<AppInfo>;
  /** Starts an elevated copy of PhoinixDR; resolves to whether this instance is about to exit. */
  relaunchElevated(): Promise<boolean>;
  listDevices(): Promise<DeviceInfo[]>;
  inspectSource(path: string): Promise<SourceInfo>;
  findPartitions(path: string): Promise<PartitionCandidate[]>;
  onSearchEvent(cb: (e: SearchEvent) => void): Promise<Unlisten>;
  startScan(request: ScanRequest): Promise<void>;
  cancelScan(): Promise<boolean>;
  listSessions(): Promise<SessionSummary[]>;
  loadSession(path: string): Promise<SessionSummary>;
  candidates(): Promise<CandidateSummary[]>;
  candidateDetail(id: string): Promise<RecoveryCandidate>;
  preview(id: string): Promise<Preview>;
  checkDestination(destination: string): Promise<DestinationInfo>;
  recover(request: RecoverRequest): Promise<RecoverItem[]>;
  verifySource(path: string): Promise<HashVerification>;
  onVerifyEvent(cb: (e: VerifyEvent) => void): Promise<Unlisten>;
  onScanEvent(cb: (e: ScanEvent) => void): Promise<Unlisten>;
  onScanComplete(cb: (e: ScanCompletion) => void): Promise<Unlisten>;
  onRecoverEvent(cb: (e: RecoverEvent) => void): Promise<Unlisten>;
  pickImageFile(): Promise<string | null>;
  pickSessionFile(): Promise<string | null>;
  pickDirectory(): Promise<string | null>;
  pickReportFile(): Promise<string | null>;
}

function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function tauriApi(): Promise<Api> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const dialog = await import("@tauri-apps/plugin-dialog");
  const on = <T,>(name: string) => async (cb: (e: T) => void): Promise<Unlisten> =>
    listen<T>(name, (event) => cb(event.payload));
  return {
    isDemo: false,
    appInfo: () => invoke<AppInfo>("app_info"),
    relaunchElevated: () => invoke<boolean>("relaunch_elevated"),
    listDevices: () => invoke<DeviceInfo[]>("list_devices"),
    inspectSource: (path) => invoke<SourceInfo>("inspect_source", { path }),
    findPartitions: (path) => invoke<PartitionCandidate[]>("find_partitions", { path }),
    onSearchEvent: on<SearchEvent>("search-event"),
    startScan: (request) => invoke<void>("start_scan", { request }),
    cancelScan: () => invoke<boolean>("cancel_scan"),
    listSessions: () => invoke<SessionSummary[]>("list_sessions"),
    loadSession: (path) => invoke<SessionSummary>("load_session", { path }),
    candidates: () => invoke<CandidateSummary[]>("candidates"),
    candidateDetail: (id) => invoke<RecoveryCandidate>("candidate_detail", { id }),
    preview: (id) => invoke<Preview>("preview_candidate", { id }),
    checkDestination: (destination) => invoke<DestinationInfo>("check_destination", { destination }),
    recover: (request) => invoke<RecoverItem[]>("recover", { request }),
    verifySource: (path) => invoke<HashVerification>("verify_source", { path }),
    onVerifyEvent: on<VerifyEvent>("verify-event"),
    onScanEvent: on<ScanEvent>("scan-event"),
    onScanComplete: on<ScanCompletion>("scan-complete"),
    onRecoverEvent: on<RecoverEvent>("recover-event"),
    pickImageFile: async () => {
      const r = await dialog.open({
        multiple: false,
        directory: false,
        title: "Choose a disk image",
        filters: [
          { name: "Disk images", extensions: ["img", "dd", "raw", "bin", "iso", "E01", "e01", "s01", "001", "vhd", "vhdx", "vmdk"] },
          { name: "All files", extensions: ["*"] },
        ],
      });
      return typeof r === "string" ? r : null;
    },
    pickSessionFile: async () => {
      const r = await dialog.open({ multiple: false, directory: false, title: "Open a session", filters: [{ name: "PhoinixDR session", extensions: ["phx"] }] });
      return typeof r === "string" ? r : null;
    },
    pickDirectory: async () => {
      const r = await dialog.open({ multiple: false, directory: true, title: "Choose the recovery destination" });
      return typeof r === "string" ? r : null;
    },
    pickReportFile: async () => {
      const r = await dialog.save({
        title: "Save the recovery report",
        defaultPath: "recovery-report.html",
        filters: [
          { name: "HTML report", extensions: ["html"] },
          { name: "Markdown report", extensions: ["md"] },
          { name: "JSON report", extensions: ["json"] },
        ],
      });
      return typeof r === "string" ? r : null;
    },
  };
}

function demoApi(): Api {
  let rows: CandidateSummary[] = [];
  const scanListeners = new Set<(e: ScanEvent) => void>();
  const searchListeners = new Set<(e: SearchEvent) => void>();
  const completeListeners = new Set<(e: ScanCompletion) => void>();
  const recoverListeners = new Set<(e: RecoverEvent) => void>();
  let lastRequest: ScanRequest | null = null;
  return {
    isDemo: true,
    appInfo: async () => demo.demoAppInfo,
    relaunchElevated: async () => {
      throw new Error("Not available in the browser demo.");
    },
    listDevices: async () => demo.demoDevices,
    inspectSource: async (path) => demo.demoSource(path),
    findPartitions: async (path) => {
      const total = 16_357_785_600;
      for (let done = 0; done < total; done += total / 4) {
        searchListeners.forEach((cb) => cb({ kind: "progress", done, total }));
        await new Promise((r) => window.setTimeout(r, 120));
      }
      const candidates = demo.demoPartitions(path);
      searchListeners.forEach((cb) => cb({ kind: "finished", candidates }));
      return candidates;
    },
    onSearchEvent: async (cb) => {
      searchListeners.add(cb);
      return () => searchListeners.delete(cb);
    },
    startScan: async (request) => {
      rows = [];
      lastRequest = request;
      demo.demoScan(request, (e) => {
        if (e.kind === "candidates") rows = rows.concat(e.items);
        scanListeners.forEach((cb) => cb(e));
        if (e.kind === "finished") completeListeners.forEach((cb) => cb({ kind: "session", summary: e.summary, cancelled: false }));
      });
    },
    cancelScan: async () => false,
    listSessions: async () => (lastRequest ? [demo.demoSession(lastRequest, rows.length)] : []),
    loadSession: async () => {
      if (!lastRequest) throw new Error("no demo session");
      return demo.demoSession(lastRequest, rows.length);
    },
    candidates: async () => rows,
    candidateDetail: async (id) => {
      const row = rows.find((r) => r.id === id);
      if (!row) throw new Error(`candidate ${id} not found`);
      return demo.demoDetail(row);
    },
    preview: async (id) => {
      const row = rows.find((r) => r.id === id);
      if (!row) throw new Error(`candidate ${id} not found`);
      return demo.demoPreview(row);
    },
    checkDestination: async (destination) => ({ destination, same_disk: false, overwrites_source_image: false, dangerous: false, warning: null }),
    recover: async (request) => {
      recoverListeners.forEach((cb) => cb({ kind: "started", total: request.candidates.length, warning: null }));
      const items = demo.demoRecover(request.candidates, rows, request.destination);
      items.forEach((item, i) => recoverListeners.forEach((cb) => cb({ kind: "item", index: i + 1, total: items.length, item })));
      recoverListeners.forEach((cb) => cb({ kind: "finished", items, failures: 0, report: request.report ?? null }));
      return items;
    },
    verifySource: async () => ({
      bytes: 16_357_785_600,
      md5: "0ea824cc3ee46762ee75d7f54444be3f",
      sha1: "8fb910bd85ca911d5be924e53cbbd6c35bbde2f5",
      sha256: "12d6637f93c4b4067a3f493435a4dfc62de88d6ebd69fab3baf9f5d89c9b31d5",
      stored: { md5: null, sha1: null },
      md5_matches: null,
      sha1_matches: null,
    }),
    onVerifyEvent: async () => () => {},
    onScanEvent: async (cb) => {
      scanListeners.add(cb);
      return () => scanListeners.delete(cb);
    },
    onScanComplete: async (cb) => {
      completeListeners.add(cb);
      return () => completeListeners.delete(cb);
    },
    onRecoverEvent: async (cb) => {
      recoverListeners.add(cb);
      return () => recoverListeners.delete(cb);
    },
    pickImageFile: async () => window.prompt("Path of the disk image", "C:\\images\\stick.img"),
    pickSessionFile: async () => null,
    pickDirectory: async () => window.prompt("Destination directory", "D:\\recovered"),
    pickReportFile: async () => window.prompt("Report file", "D:\\recovered\\report.html"),
  };
}

let cached: Promise<Api> | null = null;

/** The API for this environment (Tauri, or the browser demo). */
export function getApi(): Promise<Api> {
  if (!cached) cached = inTauri() ? tauriApi() : Promise.resolve(demoApi());
  return cached;
}
