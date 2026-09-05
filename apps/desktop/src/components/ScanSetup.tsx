import { useState } from "react";
import type { ScanMode, ScanRequest, SourceInfo } from "../types";
import { formatBytesSi } from "../lib/format";

interface Props {
  source: SourceInfo;
  onScan: (request: ScanRequest) => void;
  onBack: () => void;
}

const BUILTIN_TYPES = ["jpeg", "png", "gif", "bmp", "pdf", "zip", "sqlite", "riff", "mp4", "7z"];

export function ScanSetup({ source, onScan, onBack }: Props) {
  const supported = source.volumes.find((v) => v.supported) ?? source.volumes[0] ?? null;
  const [partition, setPartition] = useState<number | null>(supported?.partition ?? null);
  const [mode, setMode] = useState<ScanMode>("quick");
  const [examine, setExamine] = useState(true);
  const [wholeVolume, setWholeVolume] = useState(false);
  const [types, setTypes] = useState<string[]>([]);
  const volume = source.volumes.find((v) => v.partition === partition) ?? supported;
  const needsDeep = volume ? !volume.supported : true;

  const toggleType = (t: string): void =>
    setTypes((cur) => (cur.includes(t) ? cur.filter((x) => x !== t) : [...cur, t]));

  const submit = (): void =>
    onScan({
      source: source.path,
      partition,
      mode: needsDeep ? "deep" : mode,
      examine_content: examine,
      carve: { whole_volume: wholeVolume, types, min_size: 0, alignment: 0 },
    });

  return (
    <div className="panel">
      <div className="row-between">
        <h2>How should PhoinixDR search?</h2>
        <button className="link" onClick={onBack}>Back</button>
      </div>
      <p className="mono muted">{source.path} · {formatBytesSi(source.size)} · {source.scheme === "None" ? "no partition table" : `${source.scheme} partition table`}</p>
      {source.volumes.length > 1 && (
        <fieldset>
          <legend>Volume</legend>
          {source.volumes.map((v) => (
            <label key={v.partition ?? "bare"} className="option">
              <input type="radio" name="partition" checked={partition === v.partition} onChange={() => setPartition(v.partition)} />
              <span>
                <strong>{v.partition !== null ? `Partition ${v.partition}` : "Whole source"}</strong> · {v.type_description} · {formatBytesSi(v.length)} ·{" "}
                {v.filesystem === "Unknown" ? "no recognised filesystem" : `${v.filesystem} (${v.confidence}%)`}
                {!v.supported && <span className="muted"> · deep scan only</span>}
              </span>
            </label>
          ))}
        </fieldset>
      )}
      {source.volumes.length === 1 && volume && (
        <p>
          Volume: {volume.type_description} · {formatBytesSi(volume.length)} ·{" "}
          {volume.filesystem === "Unknown" ? "no recognised filesystem" : `${volume.filesystem} (${volume.confidence}% confidence)`}
        </p>
      )}
      <fieldset>
        <legend>Mode</legend>
        <label className="option">
          <input type="radio" name="mode" checked={!needsDeep && mode === "quick"} disabled={needsDeep} onChange={() => setMode("quick")} />
          <span><strong>Quick Scan</strong><br />Deleted files and filesystem records</span>
        </label>
        <label className="option">
          <input type="radio" name="mode" checked={needsDeep || mode === "deep"} onChange={() => setMode("deep")} />
          <span><strong>Deep Scan</strong><br />Also search the unallocated space for files by signature (slower: reads the free space once)</span>
        </label>
        {needsDeep && <p className="muted">This volume has no recognised filesystem, so only a deep scan of the raw volume is possible.</p>}
      </fieldset>
      {(needsDeep || mode === "deep") && (
        <fieldset>
          <legend>Deep scan options</legend>
          <label className="option">
            <input type="checkbox" checked={wholeVolume} onChange={(e) => setWholeVolume(e.target.checked)} disabled={needsDeep} />
            <span>Carve the whole volume, not only its free space (finds files hidden inside allocated space; slower, more duplicates)</span>
          </label>
          <div className="chips">
            <span className="muted">File types:</span>
            {BUILTIN_TYPES.map((t) => (
              <button key={t} className={`chip ${types.length === 0 || types.includes(t) ? "on" : ""}`} onClick={() => toggleType(t)} type="button">{t}</button>
            ))}
            {types.length > 0 && <button className="link" type="button" onClick={() => setTypes([])}>all</button>}
          </div>
        </fieldset>
      )}
      <fieldset>
        <legend>Assessment</legend>
        <label className="option">
          <input type="checkbox" checked={examine} onChange={(e) => setExamine(e.target.checked)} />
          <span>Examine content (validates file structures; slower but raises assessment confidence)</span>
        </label>
      </fieldset>
      <div className="actions">
        <button className="primary" onClick={submit} disabled={!volume}>Scan</button>
      </div>
    </div>
  );
}
