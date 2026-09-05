import type { AppInfo, SessionSummary } from "../types";
import { formatUnix, fsLabel } from "../lib/format";

interface Props {
  info: AppInfo | null;
  sessions: SessionSummary[];
  onPhysical: () => void;
  onRemovable: () => void;
  onImage: () => void;
  onOpenSession: (s: SessionSummary) => void;
  onBrowseSession: () => void;
}

export function Home({ info, sessions, onPhysical, onRemovable, onImage, onOpenSession, onBrowseSession }: Props) {
  return (
    <div className="home">
      <div className="hero">
        <h1>PhoinixDR</h1>
        <p className="muted">Evidence-driven data recovery. Nothing is written to the source, ever.</p>
      </div>
      <h2>Recover files from</h2>
      <div className="choices">
        <button className="choice" onClick={onPhysical} disabled={info ? !info.device_access : false}>
          <strong>Physical disk</strong>
          <span>Internal drives and SSDs</span>
        </button>
        <button className="choice" onClick={onRemovable} disabled={info ? !info.device_access : false}>
          <strong>Removable device</strong>
          <span>USB sticks, SD cards, external disks</span>
        </button>
        <button className="choice" onClick={onImage}>
          <strong>Disk image</strong>
          <span>Raw / DD image files</span>
        </button>
      </div>
      {info && !info.device_access && (
        <p className="warn">Devices cannot be enumerated: run PhoinixDR with administrative privileges to scan physical disks. Disk images work without them.</p>
      )}
      <div className="row-between">
        <h2>Recent sessions</h2>
        <button className="link" onClick={onBrowseSession}>Open a session file…</button>
      </div>
      {sessions.length === 0 ? (
        <p className="muted">No sessions yet. Sessions are saved automatically after every scan{info ? ` in ${info.sessions_dir}` : ""}.</p>
      ) : (
        <table className="sessions">
          <thead>
            <tr><th>Source</th><th>Filesystem</th><th>Mode</th><th>Started</th><th>Candidates</th><th /></tr>
          </thead>
          <tbody>
            {sessions.map((s) => (
              <tr key={s.id}>
                <td className="mono">{s.source}{s.partition !== null ? ` (partition ${s.partition})` : ""}</td>
                <td>{fsLabel(s.filesystem)}</td>
                <td>{s.mode === "deep" ? "Deep" : "Quick"}{s.complete ? "" : " (partial)"}</td>
                <td>{formatUnix(s.started)}</td>
                <td>{s.candidates}</td>
                <td><button onClick={() => onOpenSession(s)}>Open</button></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
