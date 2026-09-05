// The bridge to the Tauri shell. Every call is a typed command of
// apps/desktop/src-tauri/src/commands.rs; outside Tauri (plain browser) a
// demo implementation keeps the UI usable for development.
import type {
  AppInfo,
  CandidateSummary,
  DestinationInfo,
  DeviceInfo,
  Preview,
  RecoverEvent,
  RecoverItem,
  RecoverRequest,
  RecoveryCandidate,
  ScanCompletion,
  ScanEvent,
  ScanRequest,
  SessionSummary,
  SourceInfo,
} from "./types";
import * as demo from "./demo";

export type Unlisten = () => void;

export interface Api {
  isDemo: boolean;
  appInfo(): Promise<AppInfo>;
  listDevices(): Promise<DeviceInfo[]>;
  inspectSource(path: string): Promise<SourceInfo>;
  startScan(request: ScanRequest): Promise<void>;
  cancelScan(): Promise<boolean>;
  listSessions(): Promise<SessionSummary[]>;
  loadSession(path: string): Promise<SessionSummary>;
  candidates(): Promise<CandidateSummary[]>;
  candidateDetail(id: string): Promise<RecoveryCandidate>;
  preview(id: string): Promise<Preview>;
  checkDestination(destination: string): Promise<DestinationInfo>;
  recover(request: RecoverRequest): Promise<RecoverItem[]>;
  onScanEvent(cb: (e: ScanEvent) => void): Promise<Unlisten>;
  onScanComplete(cb: (e: ScanCompletion) => void): Promise<Unlisten>;
  onRecoverEvent(cb: (e: RecoverEvent) => void): Promise<Unlisten>;
  pickImageFile(): Promise<string | null>;
  pickSessionFile(): Promise<string | null>;
  pickDirectory(): Promise<string | null>;
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
    listDevices: () => invoke<DeviceInfo[]>("list_devices"),
    inspectSource: (path) => invoke<SourceInfo>("inspect_source", { path }),
    startScan: (request) => invoke<void>("start_scan", { request }),
    cancelScan: () => invoke<boolean>("cancel_scan"),
    listSessions: () => invoke<SessionSummary[]>("list_sessions"),
    loadSession: (path) => invoke<SessionSummary>("load_session", { path }),
    candidates: () => invoke<CandidateSummary[]>("candidates"),
    candidateDetail: (id) => invoke<RecoveryCandidate>("candidate_detail", { id }),
    preview: (id) => invoke<Preview>("preview_candidate", { id }),
    checkDestination: (destination) => invoke<DestinationInfo>("check_destination", { destination }),
    recover: (request) => invoke<RecoverItem[]>("recover", { request }),
    onScanEvent: on<ScanEvent>("scan-event"),
    onScanComplete: on<ScanCompletion>("scan-complete"),
    onRecoverEvent: on<RecoverEvent>("recover-event"),
    pickImageFile: async () => {
      const r = await dialog.open({ multiple: false, directory: false, title: "Choose a disk image" });
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
  };
}

function demoApi(): Api {
  let rows: CandidateSummary[] = [];
  const scanListeners = new Set<(e: ScanEvent) => void>();
  const completeListeners = new Set<(e: ScanCompletion) => void>();
  const recoverListeners = new Set<(e: RecoverEvent) => void>();
  let lastRequest: ScanRequest | null = null;
  return {
    isDemo: true,
    appInfo: async () => demo.demoAppInfo,
    listDevices: async () => demo.demoDevices,
    inspectSource: async (path) => demo.demoSource(path),
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
      recoverListeners.forEach((cb) => cb({ kind: "finished", items, failures: 0 }));
      return items;
    },
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
  };
}

let cached: Promise<Api> | null = null;

/** The API for this environment (Tauri, or the browser demo). */
export function getApi(): Promise<Api> {
  if (!cached) cached = inTauri() ? tauriApi() : Promise.resolve(demoApi());
  return cached;
}
