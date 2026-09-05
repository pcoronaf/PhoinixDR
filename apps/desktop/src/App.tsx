import { useCallback, useEffect, useRef, useState } from "react";
import { getApi, type Api } from "./api";
import type { AppInfo, CandidateSummary, DeviceInfo, ScanCompletion, ScanEvent, ScanRequest, SessionSummary, SourceInfo } from "./types";
import { Home } from "./components/Home";
import { SourcePicker } from "./components/SourcePicker";
import { ScanSetup } from "./components/ScanSetup";
import { ScanProgress, type ProgressState } from "./components/ScanProgress";
import { Results } from "./components/Results";
import { RecoverDialog } from "./components/RecoverDialog";
import logoMark from "./assets/logo-mark.png";

type View =
  | { name: "home" }
  | { name: "devices"; removable: boolean }
  | { name: "setup"; source: SourceInfo }
  | { name: "scanning" }
  | { name: "results"; session: SessionSummary };

const EMPTY_PROGRESS: ProgressState = { phase: null, done: 0, total: null, candidates: 0, message: null };

export default function App() {
  const [api, setApi] = useState<Api | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [view, setView] = useState<View>({ name: "home" });
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [devicesLoading, setDevicesLoading] = useState(false);
  const [devicesError, setDevicesError] = useState<string | null>(null);
  const [progress, setProgress] = useState<ProgressState>(EMPTY_PROGRESS);
  const [cancelling, setCancelling] = useState(false);
  const [rows, setRows] = useState<CandidateSummary[]>([]);
  const [advanced, setAdvanced] = useState(false);
  const [recovering, setRecovering] = useState<string[] | null>(null);
  const [lastSource, setLastSource] = useState<SourceInfo | null>(null);
  const [banner, setBanner] = useState<string | null>(null);
  const rowsRef = useRef<CandidateSummary[]>([]);

  useEffect(() => {
    getApi().then(async (a) => {
      setApi(a);
      setInfo(await a.appInfo().catch(() => null));
      setSessions(await a.listSessions().catch(() => []));
    });
  }, []);

  // Scan events.
  useEffect(() => {
    if (!api) return;
    let unlistenEvent: (() => void) | null = null;
    let unlistenComplete: (() => void) | null = null;
    api.onScanEvent((e: ScanEvent) => {
      switch (e.kind) {
        case "phase":
          setProgress((p) => ({ ...p, phase: e.phase, done: 0, total: null }));
          break;
        case "progress":
          setProgress((p) => ({ ...p, phase: e.phase, done: e.done, total: e.total, candidates: e.candidates }));
          break;
        case "candidates":
          rowsRef.current = rowsRef.current.concat(e.items);
          setRows(rowsRef.current);
          setProgress((p) => ({ ...p, candidates: rowsRef.current.length }));
          break;
        case "failed":
          setProgress((p) => ({ ...p, message: e.message }));
          break;
        default:
          break;
      }
    }).then((u) => {
      unlistenEvent = u;
    });
    api.onScanComplete(async (c: ScanCompletion) => {
      setCancelling(false);
      if (c.kind === "failed") {
        setProgress((p) => ({ ...p, message: c.message }));
        return;
      }
      // The authoritative list comes from the saved session (deduplicated).
      const list = await api.candidates().catch(() => rowsRef.current);
      rowsRef.current = list;
      setRows(list);
      setSessions(await api.listSessions().catch(() => []));
      setBanner(c.cancelled ? "The scan was cancelled; showing what was found so far." : null);
      setView({ name: "results", session: c.summary });
    }).then((u) => {
      unlistenComplete = u;
    });
    return () => {
      if (unlistenEvent) unlistenEvent();
      if (unlistenComplete) unlistenComplete();
    };
  }, [api]);

  const loadDevices = useCallback(async (removable: boolean) => {
    if (!api) return;
    setView({ name: "devices", removable });
    setDevicesLoading(true);
    setDevicesError(null);
    try {
      setDevices(await api.listDevices());
    } catch (e) {
      setDevicesError(String(e));
    }
    setDevicesLoading(false);
  }, [api]);

  const inspect = useCallback(async (path: string) => {
    if (!api) return;
    setBanner(null);
    try {
      const source = await api.inspectSource(path);
      setLastSource(source);
      setView({ name: "setup", source });
    } catch (e) {
      setBanner(`Could not open ${path}: ${String(e)}`);
    }
  }, [api]);

  const startScan = useCallback(async (request: ScanRequest) => {
    if (!api) return;
    rowsRef.current = [];
    setRows([]);
    setProgress(EMPTY_PROGRESS);
    setBanner(null);
    setView({ name: "scanning" });
    try {
      await api.startScan(request);
    } catch (e) {
      setProgress((p) => ({ ...p, message: String(e) }));
    }
  }, [api]);

  const openSession = useCallback(async (s: SessionSummary) => {
    if (!api || !s.file) return;
    try {
      const summary = await api.loadSession(s.file);
      const list = await api.candidates();
      rowsRef.current = list;
      setRows(list);
      setBanner(null);
      setView({ name: "results", session: summary });
    } catch (e) {
      setBanner(String(e));
    }
  }, [api]);

  if (!api) return <div className="app"><p className="muted center">Starting…</p></div>;

  return (
    <div className="app">
      <header className="topbar">
        <button className="brand" onClick={() => setView({ name: "home" })} disabled={view.name === "scanning"}><img src={logoMark} alt="" width="22" height="22" /> PhoinixDR</button>
        <span className="crumb">{view.name === "home" ? "" : view.name === "devices" ? "Choose a source" : view.name === "setup" ? "Scan" : view.name === "scanning" ? "Scanning" : "Results"}</span>
        <span className="spacer" />
        {api.isDemo && <span className="tag">browser demo</span>}
        <span className="byline muted">{info ? `v${info.version} · by ${info.author ?? "@pcoronaf"}` : "by @pcoronaf"}</span>
        <label className="option small"><input type="checkbox" checked={advanced} onChange={(e) => setAdvanced(e.target.checked)} /><span>Advanced</span></label>
      </header>
      {banner && <div className="banner">{banner} <button className="link" onClick={() => setBanner(null)}>dismiss</button></div>}
      <main>
        {view.name === "home" && (
          <Home
            api={api}
            info={info}
            sessions={sessions}
            onPhysical={() => loadDevices(false)}
            onRemovable={() => loadDevices(true)}
            onImage={async () => {
              const p = await api.pickImageFile();
              if (p) inspect(p);
            }}
            onOpenSession={openSession}
            onBrowseSession={async () => {
              const p = await api.pickSessionFile();
              if (p) openSession({ ...sessions[0]!, file: p } as SessionSummary);
            }}
          />
        )}
        {view.name === "devices" && (
          <SourcePicker api={api} info={info} devices={devices} removableOnly={view.removable} loading={devicesLoading} error={devicesError} onChoose={(d) => inspect(d.path)} onBack={() => setView({ name: "home" })} onRefresh={() => loadDevices(view.removable)} />
        )}
        {view.name === "setup" && <ScanSetup api={api} source={view.source} onScan={startScan} onBack={() => setView({ name: "home" })} />}
        {view.name === "scanning" && (
          <ScanProgress
            state={progress}
            cancelling={cancelling}
            onCancel={async () => {
              setCancelling(true);
              await api.cancelScan();
            }}
          />
        )}
        {view.name === "results" && <Results api={api} session={view.session} rows={rows} advanced={advanced} onRecover={(ids) => setRecovering(ids)} onNewScan={() => setView({ name: "home" })} />}
      </main>
      {recovering && <RecoverDialog api={api} rows={rows} ids={recovering} acquisition={lastSource?.container?.acquisition ?? null} onClose={() => setRecovering(null)} />}
    </div>
  );
}
