import type { EngineLogLine, ScanPhase } from "../types";
import { formatBytes, percent } from "../lib/format";
import { CommandLine, EngineLogPane } from "./Advanced";

export interface ProgressState {
  phase: ScanPhase | null;
  done: number;
  total: number | null;
  candidates: number;
  message: string | null;
}

const LABEL: Record<ScanPhase, string> = {
  opening: "Opening the source",
  metadata: "Reading filesystem metadata",
  carving: "Carving unallocated space",
  finishing: "Finishing",
};

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
  const pct = state.phase === "carving" ? percent(state.done, state.total) : "";
  return (
    <div className="panel progress">
      <h2>{state.phase ? LABEL[state.phase] : "Starting"}…</h2>
      <div className="bar"><div className="fill" style={{ width: state.phase === "carving" && state.total ? `${Math.min(100, (state.done / state.total) * 100)}%` : state.phase === "finishing" ? "100%" : "0%" }} /></div>
      <p className="muted">
        {state.phase === "carving" && state.total ? `${formatBytes(state.done)} of ${formatBytes(state.total)} (${pct})` : state.phase === "metadata" ? `${state.done} records examined` : ""}
        {" · "}
        {state.candidates} candidate{state.candidates === 1 ? "" : "s"} so far
      </p>
      {state.message && <p className="error">{state.message}</p>}
      <div className="actions">
        <button onClick={onCancel} disabled={cancelling}>{cancelling ? "Cancelling…" : "Cancel"}</button>
      </div>
      {advanced && command && <CommandLine title="Equivalent command line" commands={[command]} />}
      {advanced && <EngineLogPane lines={log} />}
    </div>
  );
}
