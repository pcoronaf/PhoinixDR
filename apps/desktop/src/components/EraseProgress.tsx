import type { EraseSummary, ResumePoint, TargetDescription } from "../types";
import { formatBytes, percent } from "../lib/format";
import { etaText } from "../lib/erase";

export interface EraseState {
  description: TargetDescription | null;
  method: string;
  pass: number;
  passes: number;
  pattern: string;
  done: number;
  total: number;
  bytesPerSec: number;
  unwritable: number;
  verifying: { done: number; total: number } | null;
  summary: EraseSummary | null;
  resume: ResumePoint | null;
  message: string | null;
  ended: "finished" | "cancelled" | "failed" | null;
}

export const EMPTY_ERASE: EraseState = {
  description: null,
  method: "",
  pass: 0,
  passes: 0,
  pattern: "",
  done: 0,
  total: 0,
  bytesPerSec: 0,
  unwritable: 0,
  verifying: null,
  summary: null,
  resume: null,
  message: null,
  ended: null,
};

interface Props {
  state: EraseState;
  cancelling: boolean;
  onCancel: () => void;
  onResume: () => void;
  onDone: () => void;
}

/** Pass-by-pass progress, then the verdict. */
export function EraseProgress({
  state,
  cancelling,
  onCancel,
  onResume,
  onDone,
}: Props) {
  const d = state.description;
  const title = d
    ? `${d.device?.display_name ?? d.path}${d.partition ? ` · partition ${d.partition.index}` : ""}`
    : "";
  const fraction = state.verifying
    ? state.verifying.total
      ? state.verifying.done / state.verifying.total
      : 0
    : state.total
      ? state.done / state.total
      : 0;
  return (
    <div className="panel progress erase">
      <h2>
        {state.ended === null
          ? state.verifying
            ? "Verifying…"
            : state.passes > 0
              ? `Erasing · pass ${state.pass} of ${state.passes} (${state.pattern})…`
              : "Starting…"
          : state.ended === "finished"
            ? "Erase completed"
            : state.ended === "cancelled"
              ? "Erase cancelled"
              : "Erase failed"}
      </h2>
      <p className="mono muted">
        {title}
        {d?.device?.serial ? ` · S/N ${d.device.serial}` : ""} · {state.method}
      </p>
      <div className="bar">
        <div
          className="fill"
          style={{ width: `${Math.min(100, fraction * 100)}%` }}
        />
      </div>
      {state.ended === null && (
        <p className="muted">
          {state.verifying
            ? `${state.verifying.done} of ${state.verifying.total} blocks read back`
            : state.total
              ? `${formatBytes(state.done)} of ${formatBytes(state.total)} (${percent(state.done, state.total)}) at ${formatBytes(state.bytesPerSec)}/s · ${etaText(state.total - state.done + (state.passes - state.pass) * state.total, state.bytesPerSec)}`
              : ""}
          {state.unwritable > 0
            ? ` · ${formatBytes(state.unwritable)} refused by the device (not erased)`
            : ""}
        </p>
      )}
      {state.message && <p className="error">{state.message}</p>}
      {state.summary && (
        <div
          className={
            state.ended === "finished" &&
            state.summary.unwritable_bytes === 0 &&
            (state.summary.verification === null ||
              state.summary.verification.passed ||
              state.summary.verification.mode === "none")
              ? "warn ok"
              : "danger"
          }
        >
          <p>
            <strong>{state.summary.verdict}</strong>
          </p>
          <p className="muted">
            {state.summary.passes_completed} of {state.summary.passes} pass(es)
            · {formatBytes(state.summary.bytes_written)} written ·{" "}
            {Math.round(state.summary.seconds / 60)} min
            {state.summary.certificate
              ? ` · certificate: ${state.summary.certificate}`
              : ""}
          </p>
        </div>
      )}
      <div className="actions">
        {state.ended === null && (
          <button onClick={onCancel} disabled={cancelling}>
            {cancelling ? "Cancelling after this block…" : "Cancel"}
          </button>
        )}
        {state.ended === "cancelled" && state.resume && (
          <button className="primary" onClick={onResume}>
            Resume from pass {state.resume.pass + 1},{" "}
            {formatBytes(state.resume.offset)}
          </button>
        )}
        {state.ended !== null && <button onClick={onDone}>Done</button>}
      </div>
    </div>
  );
}
