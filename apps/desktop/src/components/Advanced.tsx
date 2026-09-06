import { useEffect, useRef, useState } from "react";
import type { EngineLogLine } from "../types";
import { copyText } from "../lib/cli";

/** A button that copies text and confirms briefly. */
export function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  const [state, setState] = useState<"idle" | "done" | "failed">("idle");
  useEffect(() => {
    if (state === "idle") return;
    const t = window.setTimeout(() => setState("idle"), 1500);
    return () => window.clearTimeout(t);
  }, [state]);
  return (
    <button className="link" onClick={async () => setState((await copyText(text)) ? "done" : "failed")}>
      {state === "done" ? "Copied" : state === "failed" ? "Copy failed" : label}
    </button>
  );
}

/** The command-line equivalent of what the desktop is doing. */
export function CommandLine({ title, commands }: { title: string; commands: string[] }) {
  return (
    <div className="cli">
      <div className="row-between">
        <h4>{title}</h4>
        <CopyButton text={commands.join("\n")} />
      </div>
      <pre>{commands.join("\n")}</pre>
    </div>
  );
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}

/** Renders log lines as the command line prints them with `-v`. */
export function logText(lines: EngineLogLine[]): string {
  return lines.map((l) => `${formatTime(l.time)} ${l.level.toUpperCase().padEnd(5)} ${l.target}: ${l.message}`).join("\n");
}

/** The live engine log: what the command line would print with `-v`. */
export function EngineLogPane({ lines, title = "Engine log" }: { lines: EngineLogLine[]; title?: string }) {
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines.length]);
  return (
    <div className="cli">
      <div className="row-between">
        <h4>{title} <span className="muted">({lines.length} line{lines.length === 1 ? "" : "s"}, as <code>phoinix -vv</code> prints them)</span></h4>
        <CopyButton text={logText(lines)} label="Copy log" />
      </div>
      <pre className="log" ref={ref}>
        {lines.length === 0 && <span className="muted">Waiting for the engine…</span>}
        {lines.map((l, i) => (
          <div key={i} className={`lvl-${l.level}`}>
            <span className="muted">{formatTime(l.time)}</span> <span className="lvl">{l.level.toUpperCase().padEnd(5)}</span> <span className="muted">{l.target}:</span> {l.message}
          </div>
        ))}
      </pre>
    </div>
  );
}
