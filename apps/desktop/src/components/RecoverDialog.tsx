import { useEffect, useState } from "react";
import type { Api } from "../api";
import type { AcquisitionInfo, CandidateSummary, DestinationInfo, RecoverEvent, RecoverItem } from "../types";
import { DISCLAIMER, formatBytes } from "../lib/format";

interface Props {
  api: Api;
  rows: CandidateSummary[];
  ids: string[];
  acquisition?: AcquisitionInfo | null;
  onClose: () => void;
}

type Stage = "setup" | "running" | "done";

export function RecoverDialog({ api, rows, ids, acquisition, onClose }: Props) {
  const [destination, setDestination] = useState("");
  const [info, setInfo] = useState<DestinationInfo | null>(null);
  const [preserveTree, setPreserveTree] = useState(true);
  const [timestamps, setTimestamps] = useState(true);
  const [hash, setHash] = useState(true);
  const [override, setOverride] = useState(false);
  const [report, setReport] = useState("");
  const [verifySource, setVerifySource] = useState(false);
  const [caseNumber, setCaseNumber] = useState(acquisition?.case_number ?? "");
  const [evidenceNumber, setEvidenceNumber] = useState(acquisition?.evidence_number ?? "");
  const [examiner, setExaminer] = useState(acquisition?.examiner ?? "");
  const [notes, setNotes] = useState(acquisition?.notes ?? "");
  const [verifying, setVerifying] = useState<{ done: number; total: number } | null>(null);
  const [reportWritten, setReportWritten] = useState<string | null>(null);
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
        setVerifying(null);
        setProgress({ index: e.index, total: e.total });
        setItems((cur) => [...cur, e.item]);
      } else if (e.kind === "verifying") {
        setVerifying({ done: e.done, total: e.total });
      } else if (e.kind === "finished") {
        setVerifying(null);
        setReportWritten(e.report ?? null);
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

  const pickReport = async (): Promise<void> => {
    const file = await api.pickReportFile();
    if (file) setReport(file);
  };

  const text = (s: string): string | null => (s.trim() ? s.trim() : null);

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
        case: { case_number: text(caseNumber), evidence_number: text(evidenceNumber), examiner: text(examiner), notes: text(notes) },
        report: text(report),
        verify_source: verifySource && Boolean(text(report)),
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
            <fieldset>
              <legend>Report and case</legend>
              <div className="dest">
                <input type="text" placeholder="Recovery report (.html, .md or .json); leave empty for none" value={report} onChange={(e) => setReport(e.target.value)} />
                <button type="button" onClick={pickReport}>Choose…</button>
              </div>
              <label className="option"><input type="checkbox" checked={verifySource} disabled={!text(report)} onChange={(e) => setVerifySource(e.target.checked)} /><span>Hash the whole source for the report (compares with the hashes an E01 stores; reads the entire image)</span></label>
              <div className="case-grid">
                <input type="text" placeholder="Case number" value={caseNumber} onChange={(e) => setCaseNumber(e.target.value)} />
                <input type="text" placeholder="Evidence number" value={evidenceNumber} onChange={(e) => setEvidenceNumber(e.target.value)} />
                <input type="text" placeholder="Examiner" value={examiner} onChange={(e) => setExaminer(e.target.value)} />
                <input type="text" placeholder="Notes" value={notes} onChange={(e) => setNotes(e.target.value)} />
              </div>
            </fieldset>
            {error && <p className="error">{error}</p>}
            <p className="disclaimer">{DISCLAIMER}</p>
            <div className="actions">
              <button className="primary" disabled={blocked} onClick={start}>Recover</button>
            </div>
          </>
        )}
        {stage === "running" && (
          <div className="progress">
            <p>{verifying ? `Hashing the source… ${verifying.total ? Math.round((verifying.done / verifying.total) * 100) : 0}%` : `Recovering${progress ? ` ${progress.index} of ${progress.total}` : ""}…`}</p>
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
            {reportWritten && <p className="mono muted">Report written to {reportWritten}</p>}
            <div className="actions"><button className="primary" onClick={onClose}>Done</button></div>
          </>
        )}
      </div>
    </div>
  );
}
