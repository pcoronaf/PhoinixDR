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
  scanning: { phase: string; done: number; total: number | null; candidates: number } | null;
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
  scanning: null,
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
    ? d.files
      ? `${d.files.entries.length} file(s) under ${d.path}`
      : `${d.space?.mount ? `${d.space.mount} ` : ""}${d.device?.display_name ?? d.path}${d.partition ? ` · partition ${d.partition.index}` : ""}${d.space ? ` · ${d.space.filesystem} free space${d.space.slack ? " + slack" : ""}` : ""}`
    : "";
  const fraction = state.scanning
    ? state.scanning.total
      ? state.scanning.done / state.scanning.total
      : 0
    : state.verifying
      ? state.verifying.total
        ? state.verifying.done / state.verifying.total
        : 0
      : state.total
        ? state.done / state.total
        : 0;
  const scanOk =
    state.summary?.scan === null ||
    state.summary?.scan === undefined ||
    !state.summary.scan.ran ||
    state.summary.scan.passed;
  return (
    <div className="panel progress erase">
      <h2>
        {state.ended === null
          ? state.scanning
            ? `Scanning the erased space (${state.scanning.phase})…`
            : state.verifying
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
          {state.scanning
            ? `${state.scanning.candidates} candidate(s) so far${state.scanning.total ? ` · ${percent(state.scanning.done, state.scanning.total)}` : ""}`
            : state.verifying
            ? `${state.verifying.done} of ${state.verifying.total} blocks read back`
            : state.total === 100 && state.bytesPerSec === 0 && state.pattern.startsWith("Firmware")
              ? `The device is working: ${state.done}% reported`
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
            scanOk &&
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
            {state.summary.bytes_written > 0 ? ` · ${formatBytes(state.summary.bytes_written)} written` : ""} ·{" "}
            {Math.round(state.summary.seconds / 60)} min
            {state.summary.certificate
              ? ` · certificate: ${state.summary.certificate}`
              : ""}
          </p>
          {state.summary.scan && state.summary.scan.ran && (
            <p className="muted">
              Post-erase {state.summary.scan.mode} scan: filesystem{" "}
              {state.summary.scan.filesystem ?? "none"} ·{" "}
              {state.summary.scan.candidates} deleted record(s),{" "}
              {state.summary.scan.with_content} with content ·{" "}
              {state.summary.scan.carved} carved file(s)
              {state.summary.scan.note ? ` · ${state.summary.scan.note}` : ""}
            </p>
          )}
          {state.summary.files_failed.length > 0 && (
            <ul className="file-list">
              {state.summary.files_failed.map((f) => (
                <li key={f.path}>
                  NOT erased: <span className="mono">{f.path}</span> · {f.error}
                </li>
              ))}
            </ul>
          )}
          {state.summary.scan && !state.summary.scan.ran && (
            <p className="muted">Post-erase scan not run{state.summary.scan.note ? `: ${state.summary.scan.note}` : ""}.</p>
          )}
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
