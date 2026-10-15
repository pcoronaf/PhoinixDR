import { useCallback, useEffect, useRef, useState } from "react";
import { getApi, type Api } from "./api";
import type { AppInfo, CandidateSummary, DeviceInfo, EngineLogLine, EraseCatalogue, EraseEvent, EraseRequest, EraseTargetInfo, MountedVolumeInfo, ScanCompletion, ScanEvent, ScanRequest, SessionSummary, SourceInfo, TargetDescription } from "./types";
import { Home } from "./components/Home";
import { SourcePicker } from "./components/SourcePicker";
import { ScanSetup } from "./components/ScanSetup";
import { ScanProgress, type ProgressState } from "./components/ScanProgress";
import { scanCommand } from "./lib/cli";
import { EraseTargets } from "./components/EraseTargets";
import { EraseSetup } from "./components/EraseSetup";
import { EMPTY_ERASE, EraseProgress, type EraseState } from "./components/EraseProgress";
import { Results } from "./components/Results";
import { RecoverDialog } from "./components/RecoverDialog";
import logoMark from "./assets/logo-mark.png";

type View =
  | { name: "home" }
  | { name: "devices"; removable: boolean }
  | { name: "setup"; source: SourceInfo }
  | { name: "scanning" }
  | { name: "results"; session: SessionSummary }
  | { name: "erase-targets" }
  | { name: "erase-setup"; description: TargetDescription }
  | { name: "erasing" };

const EMPTY_PROGRESS: ProgressState = { phase: null, done: 0, total: null, candidates: 0, bytesRead: null, unreadable: null, message: null };

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
  const [lastDevice, setLastDevice] = useState<DeviceInfo | null>(null);
  // Secure erase (M12).
  const [eraseCatalogue, setEraseCatalogue] = useState<EraseCatalogue | null>(null);
  const [eraseTargets, setEraseTargets] = useState<EraseTargetInfo[]>([]);
  const [eraseVolumes, setEraseVolumes] = useState<MountedVolumeInfo[]>([]);
  const [eraseLoading, setEraseLoading] = useState(false);
  const [eraseError, setEraseError] = useState<string | null>(null);
  const [erase, setErase] = useState<EraseState>(EMPTY_ERASE);
  const [eraseCancelling, setEraseCancelling] = useState(false);
  const [lastErase, setLastErase] = useState<EraseRequest | null>(null);
  const [lastRequest, setLastRequest] = useState<ScanRequest | null>(null);
  const [engineLog, setEngineLog] = useState<EngineLogLine[]>([]);
  const logRef = useRef<EngineLogLine[]>([]);
  const [banner, setBanner] = useState<string | null>(null);
  const rowsRef = useRef<CandidateSummary[]>([]);

  useEffect(() => {
    getApi().then(async (a) => {
      setApi(a);
      setInfo(await a.appInfo().catch(() => null));
      setSessions(await a.listSessions().catch(() => []));
    });
  }, []);

  // Advanced mode forwards the engine log while it is on.
  useEffect(() => {
    if (!api) return;
    api.setEngineLog(advanced).catch(() => {});
  }, [api, advanced]);

  // Engine log lines (batched by the backend); keep the last 2000.
  useEffect(() => {
    if (!api) return;
    let unlisten: (() => void) | null = null;
    api.onEngineLog((lines) => {
      logRef.current = logRef.current.concat(lines).slice(-2000);
      setEngineLog(logRef.current);
    }).then((u) => {
      unlisten = u;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, [api]);

  // Erase events.
  useEffect(() => {
    if (!api) return;
    let unlisten: (() => void) | null = null;
    api.onEraseEvent((e: EraseEvent) => {
      switch (e.kind) {
        case "started":
          setErase((s) => ({ ...s, description: e.description, method: e.method, passes: e.passes, ended: null, summary: null, resume: null, message: null }));
          break;
        case "pass":
          setErase((s) => ({ ...s, pass: e.pass, passes: e.passes, pattern: e.pattern, done: 0 }));
          break;
        case "progress":
          setErase((s) => ({ ...s, pass: e.pass, passes: e.passes, done: e.done, total: e.total, bytesPerSec: e.bytes_per_sec, unwritable: e.unwritable }));
          break;
        case "verifying":
          setErase((s) => ({ ...s, verifying: { done: e.done, total: e.total } }));
          break;
        case "scanning":
          setErase((s) => ({ ...s, verifying: null, scanning: { phase: e.phase, done: e.done, total: e.total, candidates: e.candidates } }));
          break;
        case "finished":
          setEraseCancelling(false);
          setErase((s) => ({ ...s, verifying: null, scanning: null, summary: e.summary, ended: "finished" }));
          break;
        case "cancelled":
          setEraseCancelling(false);
          setErase((s) => ({ ...s, verifying: null, scanning: null, summary: e.summary, resume: e.resume, ended: "cancelled" }));
          break;
        case "failed":
          setEraseCancelling(false);
          setErase((s) => ({ ...s, verifying: null, scanning: null, summary: e.summary.passes > 0 ? e.summary : s.summary, message: e.message, ended: "failed" }));
          break;
        default:
          break;
      }
    }).then((u) => {
      unlisten = u;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, [api]);

  const loadEraseTargets = useCallback(async () => {
    if (!api) return;
    setView({ name: "erase-targets" });
    setEraseLoading(true);
    setEraseError(null);
    try {
      if (!eraseCatalogue) setEraseCatalogue(await api.eraseCatalogue());
      const [targets, volumes] = await Promise.all([api.eraseTargets(), api.eraseVolumes().catch(() => [] as MountedVolumeInfo[])]);
      setEraseTargets(targets);
      setEraseVolumes(volumes);
    } catch (e) {
      setEraseError(String(e));
    }
    setEraseLoading(false);
  }, [api, eraseCatalogue]);

  const chooseEraseFiles = useCallback(async (paths: string[]) => {
    if (!api) return;
    try {
      const description = await api.describeEraseTarget({ kind: "files", paths });
      setView({ name: "erase-setup", description });
    } catch (e) {
      setEraseError(String(e));
    }
  }, [api]);

  const chooseEraseVolume = useCallback(async (v: MountedVolumeInfo) => {
    if (!api) return;
    try {
      const description = await api.describeEraseTarget({ kind: "mounted_free_space", mount: v.mount, reserve: 0 });
      setView({ name: "erase-setup", description });
    } catch (e) {
      setEraseError(String(e));
    }
  }, [api]);

  const startErase = useCallback(async (request: EraseRequest) => {
    if (!api) return;
    setLastErase(request);
    setEraseCancelling(false);
    setErase(EMPTY_ERASE);
    setView({ name: "erasing" });
    try {
      await api.startErase(request);
    } catch (e) {
      setErase((s) => ({ ...s, message: String(e), ended: "failed" }));
    }
  }, [api]);

  // Scan events.
  useEffect(() => {
    if (!api) return;
    let unlistenEvent: (() => void) | null = null;
    let unlistenComplete: (() => void) | null = null;
    api.onScanEvent((e: ScanEvent) => {
      switch (e.kind) {
        case "phase":
          setProgress((p) => ({ ...p, phase: e.phase, done: 0, total: null, bytesRead: null }));
          break;
        case "progress":
          setProgress((p) => ({ ...p, phase: e.phase, done: e.done, total: e.total, candidates: e.candidates, bytesRead: e.bytes_read ?? null, unreadable: e.unreadable_bytes ?? p.unreadable }));
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
    setLastRequest(request);
    logRef.current = [];
    setEngineLog([]);
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
        <button className="brand" onClick={() => setView({ name: "home" })} disabled={view.name === "scanning" || view.name === "erasing"}><img src={logoMark} alt="" width="22" height="22" /> PhoinixDR</button>
        <span className="crumb">{view.name === "home" ? "" : view.name === "devices" ? "Choose a source" : view.name === "setup" ? "Scan" : view.name === "scanning" ? "Scanning" : view.name === "results" ? "Results" : view.name === "erasing" ? "Erasing" : "Erase"}</span>
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
              if (p) {
                setLastDevice(null);
                inspect(p);
              }
            }}
            onErase={loadEraseTargets}
            onOpenSession={openSession}
            onBrowseSession={async () => {
              const p = await api.pickSessionFile();
              if (p) openSession({ ...sessions[0]!, file: p } as SessionSummary);
            }}
          />
        )}
        {view.name === "erase-targets" && eraseCatalogue !== null && (
          <EraseTargets
            api={api}
            info={info}
            catalogue={eraseCatalogue}
            targets={eraseTargets}
            volumes={eraseVolumes}
            loading={eraseLoading}
            error={eraseError}
            onChoose={(t) => {
              if (t.description) setView({ name: "erase-setup", description: t.description });
            }}
            onChooseVolume={chooseEraseVolume}
            onChooseFiles={chooseEraseFiles}
            onBack={() => setView({ name: "home" })}
            onRefresh={loadEraseTargets}
          />
        )}
        {view.name === "erase-setup" && eraseCatalogue !== null && (
          <EraseSetup api={api} catalogue={eraseCatalogue} initial={view.description} onStart={startErase} onBack={loadEraseTargets} />
        )}
        {view.name === "erasing" && (
          <EraseProgress
            state={erase}
            cancelling={eraseCancelling}
            onCancel={async () => {
              setEraseCancelling(true);
              await api.cancelErase();
            }}
            onResume={() => {
              if (lastErase) startErase({ ...lastErase, resume: true });
            }}
            onDone={() => setView({ name: "home" })}
          />
        )}
        {view.name === "devices" && (
          <SourcePicker api={api} info={info} devices={devices} removableOnly={view.removable} loading={devicesLoading} error={devicesError} onChoose={(d) => {
              setLastDevice(d);
              inspect(d.path);
            }} onBack={() => setView({ name: "home" })} onRefresh={() => loadDevices(view.removable)} />
        )}
        {view.name === "setup" && <ScanSetup api={api} source={view.source} device={lastDevice} onScan={startScan} onBack={() => setView({ name: "home" })} />}
        {view.name === "scanning" && (
          <ScanProgress
            advanced={advanced}
            command={lastRequest ? scanCommand(lastRequest) : null}
            log={engineLog}
            state={progress}
            cancelling={cancelling}
            onCancel={async () => {
              setCancelling(true);
              await api.cancelScan();
            }}
          />
        )}
        {view.name === "results" && <Results api={api} session={view.session} rows={rows} advanced={advanced} request={lastRequest} log={engineLog} onRecover={(ids) => setRecovering(ids)} onNewScan={() => setView({ name: "home" })} />}
      </main>
      {recovering && <RecoverDialog api={api} rows={rows} ids={recovering} acquisition={lastSource?.container?.acquisition ?? null} onClose={() => setRecovering(null)} />}
    </div>
  );
}
