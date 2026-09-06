import type { EngineLogLine, ScanPhase } from "../types";
import { formatBytes, percent } from "../lib/format";
import { CommandLine, EngineLogPane } from "./Advanced";

export interface ProgressState {
  phase: ScanPhase | null;
  done: number;
  total: number | null;
  candidates: number;
  /** Bytes read from the source in the current phase, when reported. */
  bytesRead: number | null;
  /** Bytes the device could not read so far, when any. */
  unreadable: number | null;
  message: string | null;
}

const LABEL: Record<ScanPhase, string> = {
  opening: "Opening the source",
  metadata: "Reading filesystem metadata",
  carving: "Carving unallocated space (header search)",
  assembling: "Examining carved files",
  finishing: "Finishing",
};

/** Progress fraction of the current phase, when it has a known total. */
function fraction(state: ProgressState): number | null {
  if ((state.phase === "carving" || state.phase === "assembling") && state.total) return Math.min(1, state.done / state.total);
  if (state.phase === "finishing") return 1;
  return null;
}

function detail(state: ProgressState): string {
  switch (state.phase) {
    case "carving":
      return state.total ? `${formatBytes(state.done)} of ${formatBytes(state.total)} (${percent(state.done, state.total)})` : "";
    case "assembling":
      return state.total
        ? `hit ${state.done} of ${state.total} examined (${percent(state.done, state.total)})${state.bytesRead !== null ? `, ${formatBytes(state.bytesRead)} read` : ""}; each hit is read back from the source`
        : "";
    case "metadata":
      return `${state.done} records examined`;
    default:
      return "";
  }
}

interface Props {
  state: ProgressState;
  onCancel: () => void;
  cancelling: boolean;
  /** Advanced mode: show the equivalent command line and the live engine log. */
  advanced?: boolean;
  command?: string | null;
  log?: EngineLogLine[];
}

export function ScanProgress({ state, onCancel, cancelling, advanced = false, command = null, log = [] }: Props) {
  const f = fraction(state);
  const text = detail(state);
  return (
    <div className="panel progress">
      <h2>{state.phase ? LABEL[state.phase] : "Starting"}…</h2>
      <div className="bar"><div className="fill" style={{ width: f === null ? "0%" : `${f * 100}%` }} /></div>
      <p className="muted">
        {text}
        {text ? " · " : ""}
        {state.candidates} candidate{state.candidates === 1 ? "" : "s"} so far
      </p>
      {state.unreadable !== null && state.unreadable > 0 && (
        <p className="warn">{formatBytes(state.unreadable)} could not be read from the device so far; the regions were skipped and are treated as zeros.</p>
      )}
      {state.message && <p className="error">{state.message}</p>}
      <div className="actions">
        <button onClick={onCancel} disabled={cancelling}>{cancelling ? "Cancelling…" : "Cancel"}</button>
      </div>
      {advanced && command && <CommandLine title="Equivalent command line" commands={[command]} />}
      {advanced && <EngineLogPane lines={log} />}
    </div>
  );
}
