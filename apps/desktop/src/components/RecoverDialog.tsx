import { useEffect, useState } from "react";
import type { Api } from "../api";
import type { CandidateSummary, DestinationInfo, RecoverEvent, RecoverItem } from "../types";
import { formatBytes } from "../lib/format";

interface Props {
  api: Api;
  rows: CandidateSummary[];
  ids: string[];
  onClose: () => void;
}

type Stage = "setup" | "running" | "done";

export function RecoverDialog({ api, rows, ids, onClose }: Props) {
  const [destination, setDestination] = useState("");
  const [info, setInfo] = useState<DestinationInfo | null>(null);
  const [preserveTree, setPreserveTree] = useState(true);
  const [timestamps, setTimestamps] = useState(true);
  const [hash, setHash] = useState(true);
  const [override, setOverride] = useState(false);
  const [stage, setStage] = useState<Stage>("setup");
  const [progress, setProgress] = useState<{ index: number; total: number } | null>(null);
  const [items, setItems] = useState<RecoverItem[]>([]);
  const [error, setError] = useState<string | null>(null);
  const chosen = rows.filter((r) => ids.includes(r.id));
  const totalBytes = chosen.reduce((n, r) => n + (r.size ?? 0), 0);

  useEffect(() => {
    if (!destination) {
      setInfo(null);
      return;
    }
    let alive = true;
    api.checkDestination(destination).then((i) => alive && setInfo(i)).catch((e: unknown) => alive && setError(String(e)));
    return () => {
      alive = false;
    };
  }, [api, destination]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    api.onRecoverEvent((e: RecoverEvent) => {
      if (e.kind === "item") {
        setProgress({ index: e.index, total: e.total });
        setItems((cur) => [...cur, e.item]);
      }
    }).then((u) => {
      unlisten = u;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, [api]);

  const pick = async (): Promise<void> => {
    const dir = await api.pickDirectory();
    if (dir) setDestination(dir);
  };

  const start = async (): Promise<void> => {
    setStage("running");
    setItems([]);
    setError(null);
    try {
      const result = await api.recover({
        candidates: ids,
        destination,
        preserve_tree: preserveTree,
        preserve_timestamps: timestamps,
        hash,
        overwrite: false,
        allow_same_device: override,
      });
      setItems(result);
    } catch (e) {
      setError(String(e));
    }
    setStage("done");
  };

  const blocked = !destination || (info?.dangerous && !override) || info?.overwrites_source_image;
  const failures = items.filter((i) => i.error || (i.result && !i.result.complete)).length;

  return (
    <div className="modal-backdrop">
      <div className="modal">
        <div className="row-between">
          <h2>Recover {chosen.length} file{chosen.length === 1 ? "" : "s"} <span className="muted">({formatBytes(totalBytes)})</span></h2>
          <button className="link" onClick={onClose} disabled={stage === "running"}>Close</button>
        </div>
        {stage === "setup" && (
          <>
            <div className="dest">
              <input type="text" placeholder="Destination directory (must be on another disk)" value={destination} onChange={(e) => setDestination(e.target.value)} />
              <button onClick={pick}>Choose…</button>
            </div>
            {info?.warning && (
              <div className={info.dangerous ? "danger" : "warn"}>
                <p>{info.warning}</p>
                {info.dangerous && !info.overwrites_source_image && (
                  <label className="option">
                    <input type="checkbox" checked={override} onChange={(e) => setOverride(e.target.checked)} />
                    <span>I understand that writing here can permanently overwrite the data I am recovering (expert override).</span>
                  </label>
                )}
              </div>
            )}
            <fieldset>
              <legend>Options</legend>
              <label className="option"><input type="checkbox" checked={preserveTree} onChange={(e) => setPreserveTree(e.target.checked)} /><span>Recreate the original folder structure</span></label>
              <label className="option"><input type="checkbox" checked={timestamps} onChange={(e) => setTimestamps(e.target.checked)} /><span>Apply the original timestamps</span></label>
              <label className="option"><input type="checkbox" checked={hash} onChange={(e) => setHash(e.target.checked)} /><span>Verify every file with SHA-256</span></label>
            </fieldset>
            {error && <p className="error">{error}</p>}
            <div className="actions">
              <button className="primary" disabled={blocked} onClick={start}>Recover</button>
            </div>
          </>
        )}
        {stage === "running" && (
          <div className="progress">
            <p>Recovering{progress ? ` ${progress.index} of ${progress.total}` : ""}…</p>
            <div className="bar"><div className="fill" style={{ width: progress ? `${(progress.index / progress.total) * 100}%` : "0%" }} /></div>
          </div>
        )}
        {(stage === "running" || stage === "done") && items.length > 0 && (
          <table className="recovered">
            <thead><tr><th>File</th><th>Written</th><th>Result</th></tr></thead>
            <tbody>
              {items.map((i) => (
                <tr key={i.id} className={i.error || (i.result && !i.result.complete) ? "bad" : "good"}>
                  <td className="name">{i.name}</td>
                  <td className="num">{i.result ? formatBytes(i.result.bytes_written) : "–"}</td>
                  <td className="mono">{i.error ? i.error : i.result ? `${i.result.complete ? "verified" : "PARTIAL"}${i.result.sha256 ? ` · ${i.result.sha256.slice(0, 16)}…` : ""} → ${i.result.output_path}` : ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {stage === "done" && (
          <>
            {error && <p className="error">{error}</p>}
            <p>{failures === 0 ? "Every file was recovered and verified." : `${failures} of ${items.length} recoveries failed or were partial.`}</p>
            <div className="actions"><button className="primary" onClick={onClose}>Done</button></div>
          </>
        )}
      </div>
    </div>
  );
}
