import { useEffect, useState } from "react";
import type { Api } from "../api";
import type { PartitionCandidate, ScanMode, ScanRequest, SourceInfo, VolumeRange } from "../types";
import { formatBytesSi, fsLabel, hasEngine, percent } from "../lib/format";

interface Props {
  api: Api;
  source: SourceInfo;
  onScan: (request: ScanRequest) => void;
  onBack: () => void;
}

function relationText(c: PartitionCandidate): string {
  switch (c.relation.kind) {
    case "listed":
      return `partition ${c.relation.index}`;
    case "lost":
      return "lost partition";
    case "inside_partition":
      return `inside partition ${c.relation.index}`;
    case "nested":
      return "nested (probably an image file)";
    case "overlapping":
      return "overlaps another candidate";
    default:
      return "";
  }
}

const BUILTIN_TYPES = ["jpeg", "png", "gif", "bmp", "pdf", "zip", "sqlite", "riff", "mp4", "7z"];

export function ScanSetup({ api, source, onScan, onBack }: Props) {
  const supported = source.volumes.find((v) => v.supported) ?? source.volumes[0] ?? null;
  const [partition, setPartition] = useState<number | null>(supported?.partition ?? null);
  const [lost, setLost] = useState<PartitionCandidate[] | null>(null);
  const [lostChoice, setLostChoice] = useState<number | null>(null);
  const [searching, setSearching] = useState<{ done: number; total: number } | null>(null);
  const [searchError, setSearchError] = useState<string | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    api.onSearchEvent((e) => {
      if (e.kind === "progress") setSearching({ done: e.done, total: e.total });
    }).then((u) => {
      unlisten = u;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, [api]);

  const searchLost = async (): Promise<void> => {
    setSearching({ done: 0, total: 0 });
    setSearchError(null);
    try {
      const found = await api.findPartitions(source.path);
      setLost(found);
      const firstLost = found.findIndex((c) => c.relation.kind === "lost");
      setLostChoice(firstLost >= 0 ? firstLost : null);
    } catch (e) {
      setSearchError(String(e));
    }
    setSearching(null);
  };
  const [mode, setMode] = useState<ScanMode>("quick");
  const [examine, setExamine] = useState(true);
  const [wholeVolume, setWholeVolume] = useState(false);
  const [types, setTypes] = useState<string[]>([]);
  const chosenLost = lostChoice !== null && lost ? (lost[lostChoice] ?? null) : null;
  const volume = source.volumes.find((v) => v.partition === partition) ?? supported;
  const needsDeep = chosenLost ? !hasEngine(chosenLost.filesystem) : volume ? !volume.supported : true;

  const toggleType = (t: string): void =>
    setTypes((cur) => (cur.includes(t) ? cur.filter((x) => x !== t) : [...cur, t]));

  const submit = (): void => {
    const range: VolumeRange | null = chosenLost
      ? { offset: chosenLost.start, length: chosenLost.readable_length, repairs: chosenLost.repairs }
      : null;
    onScan({
      source: source.path,
      partition: range ? null : partition,
      volume: range,
      mode: needsDeep ? "deep" : mode,
      examine_content: examine,
      carve: { whole_volume: wholeVolume, types, min_size: 0, alignment: 0 },
    });
  };

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
                {v.filesystem === "unknown" ? "no recognised filesystem" : `${fsLabel(v.filesystem)} (${v.confidence}%)`}
                {!v.supported && <span className="muted"> · deep scan only</span>}
              </span>
            </label>
          ))}
        </fieldset>
      )}
      {source.volumes.length === 1 && volume && (
        <p>
          Volume: {volume.type_description} · {formatBytesSi(volume.length)} ·{" "}
          {volume.filesystem === "unknown" ? "no recognised filesystem" : `${fsLabel(volume.filesystem)} (${volume.confidence}% confidence)`}
        </p>
      )}
      <fieldset>
        <legend>Lost partitions</legend>
        <p className="muted">Searches the whole source for filesystem structures independently of the partition table. Nothing is written; a found volume is mounted virtually, with its backup boot sector standing in when the primary is destroyed.</p>
        {searching ? (
          <p>Searching… {percent(searching.done, searching.total || null)}</p>
        ) : (
          <button type="button" onClick={searchLost}>{lost ? "Search again" : "Search for lost partitions"}</button>
        )}
        {searchError && <p className="error">{searchError}</p>}
        {lost && lost.length === 0 && <p className="muted">No filesystem structures found.</p>}
        {lost && lost.length > 0 && (
          <div>
            <label className="option">
              <input type="radio" name="lost" checked={lostChoice === null} onChange={() => setLostChoice(null)} />
              <span>Use the partition table (above)</span>
            </label>
            {lost.map((c, i) => (
              <label key={`${c.start}-${c.filesystem}`} className="option">
                <input type="radio" name="lost" checked={lostChoice === i} onChange={() => setLostChoice(i)} />
                <span>
                  <strong>{fsLabel(c.filesystem)}</strong>{c.label ? ` “${c.label}”` : ""} · {formatBytesSi(c.length)} at offset {c.start.toLocaleString()} · {relationText(c)} · confidence {c.confidence}%
                  {c.repairs.length > 0 && <span className="muted"> · {c.repairs[0]?.description}</span>}
                  {c.relation.kind !== "lost" && c.relation.kind !== "listed" && <span className="warn"> {c.evidence.filter((e) => !e.supports).map((e) => e.description).join("; ")}</span>}
                </span>
              </label>
            ))}
          </div>
        )}
      </fieldset>
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
        <button className="primary" onClick={submit} disabled={!volume && !chosenLost}>Scan{chosenLost ? " the lost partition" : ""}</button>
      </div>
    </div>
  );
}
