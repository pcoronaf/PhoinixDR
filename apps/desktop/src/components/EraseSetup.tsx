import { useEffect, useState } from "react";
import type { Api } from "../api";
import type {
  EraseCatalogue,
  EraseRequest,
  EraseTarget,
  FirmwareAction,
  PostScan,
  TargetDescription,
  VerifyMode,
} from "../types";
import { formatBytesSi } from "../lib/format";
import { refusalText } from "../lib/erase";

interface Props {
  api: Api;
  catalogue: EraseCatalogue;
  initial: TargetDescription;
  onStart: (request: EraseRequest) => void;
  onBack: () => void;
}

/** What the page lets the user pick on a disk. */
type Scope =
  | { kind: "whole_device" }
  | { kind: "partition"; index: number }
  | { kind: "free_space"; index: number };

function text(s: string): string | null {
  const t = s.trim();
  return t.length > 0 ? t : null;
}

function scopeOf(d: TargetDescription): Scope {
  switch (d.target.kind) {
    case "partition":
      return { kind: "partition", index: d.target.index };
    case "free_space":
      return { kind: "free_space", index: d.target.partition ?? d.partition?.index ?? 1 };
    default:
      return { kind: "whole_device" };
  }
}

function sameScope(a: Scope, b: Scope): boolean {
  return a.kind === b.kind && (a.kind === "whole_device" || ("index" in a && "index" in b && a.index === b.index));
}

/** Target, method, operator, confirmation and the countdown. */
export function EraseSetup({
  api,
  catalogue,
  initial,
  onStart,
  onBack,
}: Props) {
  const mounted = initial.target.kind === "mounted_free_space";
  const filesTarget = initial.target.kind === "files";
  const [desc, setDesc] = useState<TargetDescription>(initial);
  const [scope, setScope] = useState<Scope>(scopeOf(initial));
  const [slack, setSlack] = useState(false);
  const [reserveGb, setReserveGb] = useState("0");
  const firmwareDefault = initial.firmware && initial.firmware.supports.length > 0 && initial.solid_state ? `firmware:${initial.firmware.supports[0]}` : null;
  const [method, setMethod] = useState(firmwareDefault ?? "zero");
  const [verify, setVerify] = useState<VerifyMode>("sample");
  const [postScan, setPostScan] = useState<PostScan>("none");
  const [certificate, setCertificate] = useState("");
  const [name, setName] = useState("");
  const [organisation, setOrganisation] = useState("");
  const [asset, setAsset] = useState("");
  const [notes, setNotes] = useState("");
  const [typed, setTyped] = useState("");
  const [countdown, setCountdown] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [describing, setDescribing] = useState(false);
  const firmwareAction = method.startsWith("firmware:") ? method.slice("firmware:".length) : null;
  const chosen =
    (firmwareAction ? catalogue.firmware_actions.find((m) => m.id === firmwareAction) : catalogue.methods.find((m) => m.id === method)) ?? catalogue.methods[0]!;
  const fw = desc.firmware;
  const firmwareOffered = !mounted && !filesTarget && scope.kind === "whole_device" && fw !== null && fw.supports.length > 0;
  const firmwareBlocked = (a: string): string | null => {
    if (!fw) return "not offered";
    if (fw.locked) return "the device is security-locked";
    if (fw.frozen && (a === "security" || a === "enhanced")) return "security frozen";
    return null;
  };
  const confirmed =
    typed.trim().length > 0 &&
    (typed.trim().toLowerCase() === desc.confirmation.toLowerCase() ||
      (desc.confirmation_short !== null &&
        typed.trim().toLowerCase() === desc.confirmation_short.toLowerCase()));
  const reserveBytes = Math.max(0, Math.round((parseFloat(reserveGb) || 0) * 1_000_000_000));

  // Re-describe when the scope, the slack option or the reserve changes.
  useEffect(() => {
    let target: EraseTarget;
    if (filesTarget) {
      target = initial.target;
    } else if (mounted && initial.target.kind === "mounted_free_space") {
      target = { kind: "mounted_free_space", mount: initial.target.mount, reserve: reserveBytes };
    } else if (scope.kind === "whole_device") {
      target = { kind: "whole_device", path: initial.path };
    } else if (scope.kind === "partition") {
      target = { kind: "partition", path: initial.path, index: scope.index };
    } else {
      target = { kind: "free_space", path: initial.path, partition: scope.index, slack };
    }
    let alive = true;
    setDescribing(true);
    setError(null);
    api
      .describeEraseTarget(target)
      .then((d) => {
        if (alive) setDesc(d);
      })
      .catch((e: unknown) => {
        if (alive) setError(String(e));
      })
      .finally(() => {
        if (alive) setDescribing(false);
      });
    return () => {
      alive = false;
    };
  }, [api, initial.path, initial.target, mounted, filesTarget, scope, slack, reserveBytes]);

  // Countdown.
  useEffect(() => {
    if (countdown === null) return;
    if (countdown === 0) {
      const request: EraseRequest = {
        target: desc.target,
        method: firmwareAction ? { id: "firmware", action: firmwareAction as FirmwareAction } : { id: method },
        verify,
        operator: {
          name: text(name),
          organisation: text(organisation),
          asset: text(asset),
          notes: text(notes),
        },
        certificate: text(certificate),
        confirmation: typed.trim(),
        resume: false,
        state_dir: null,
        allow_executable_disk: false,
        allow_sessions_disk: false,
        post_scan: postScan,
      };
      onStart(request);
      return;
    }
    const t = window.setTimeout(() => setCountdown(countdown - 1), 1000);
    return () => window.clearTimeout(t);
  }, [
    countdown,
    desc.target,
    method,
    firmwareAction,
    verify,
    postScan,
    name,
    organisation,
    asset,
    notes,
    certificate,
    typed,
    onStart,
  ]);

  useEffect(() => {
    if (firmwareAction && !firmwareOffered) setMethod("zero");
  }, [firmwareAction, firmwareOffered]);

  const pick = async (): Promise<void> => {
    const p = await api.pickCertificateFile();
    if (p) setCertificate(p);
  };
  const d = desc.device;
  const space = desc.space;
  const freeSpace = scope.kind === "free_space" || mounted;
  const files = desc.files;
  const what = filesTarget
    ? `${files?.entries.length ?? 0} file(s)`
    : mounted
    ? `the free space of ${desc.path}`
    : scope.kind === "whole_device"
      ? "the whole disk"
      : scope.kind === "partition"
        ? `partition ${scope.index}`
        : `the free space of partition ${scope.index}${slack ? " and the file slack" : ""}`;

  return (
    <div className="panel erase">
      <div className="row-between">
        <h2>
          Erase{" "}
          {filesTarget
            ? `${files?.entries.length ?? 0} file(s)${(files?.directories.length ?? 0) > 0 ? ` and ${files?.directories.length} folder(s)` : ""}`
            : mounted
              ? `free space of ${desc.path}`
              : d
                ? d.display_name
                : desc.path}
        </h2>
        <button className="link" onClick={onBack} disabled={countdown !== null}>
          Back
        </button>
      </div>
      <p className="mono muted">
        {filesTarget && space ? `${desc.path} · ${space.filesystem} on ${space.mount ?? ""} · ${formatBytesSi(files?.total_bytes ?? 0)} of content in ${files?.entries.length ?? 0} file(s), ${files?.stream_count ?? 0} named stream(s)` : mounted && space ? `${space.filesystem}${space.label ? ` "${space.label}"` : ""} · ${space.node ?? ""} · ${formatBytesSi(space.free_bytes)} free of ${formatBytesSi(space.total_bytes ?? space.volume_length)}` : desc.path}
        {!mounted && !filesTarget && d?.serial ? ` · S/N ${d.serial}` : ""}
        {!mounted && !filesTarget ? ` · ${formatBytesSi(desc.length)}` : ""}
        {d
          ? d.rotational === false
            ? " · solid state"
            : d.rotational
              ? " · rotational"
              : ""
          : ""}
      </p>
      {desc.solid_state && <p className="warn">{catalogue.flash_warning}</p>}
      {desc.refusals.length > 0 && (
        <div className="danger">
          {desc.refusals.map((r, i) => (
            <p key={i}>Cannot erase: this target {refusalText(r)}.</p>
          ))}
        </div>
      )}
      {error && <p className="error">{error}</p>}

      <fieldset>
        <legend>What to erase</legend>
        {filesTarget && files ? (
          <>
            <ul className="file-list">
              {files.entries.slice(0, 40).map((e) => (
                <li key={e.path}>
                  <span className="mono">{e.path}</span> · {formatBytesSi(e.size)}
                  {e.streams.length > 0 ? ` · ${e.streams.length} named stream(s)` : ""}
                  {e.read_only ? " · read-only" : ""}
                </li>
              ))}
              {files.entries.length > 40 && <li className="muted">… and {files.entries.length - 40} more</li>}
            </ul>
            <p className="muted">
              {formatBytesSi(desc.length)} will be written per pass (every stream padded to the allocation unit of {formatBytesSi(files.allocation_unit)});
              {files.directories.length > 0 ? ` ${files.directories.length} folder(s) are removed at the end.` : " the folders stay."}
            </p>
          </>
        ) : mounted ? (
          <>
            <p>
              Files are written into <span className="mono">{desc.path}</span> until the volume is full
              ({formatBytesSi(desc.length)} to write), rewritten for every further pass, read back and removed.
              Live files are never touched.
            </p>
            <label className="option">
              <span>Leave free (GB)</span>
              <input
                type="number"
                min="0"
                step="0.5"
                value={reserveGb}
                onChange={(e) => setReserveGb(e.target.value)}
                disabled={countdown !== null}
              />
            </label>
          </>
        ) : (
          <>
            <label className="option">
              <input
                type="radio"
                name="scope"
                checked={scope.kind === "whole_device"}
                onChange={() => setScope({ kind: "whole_device" })}
              />
              <span>
                <strong>The whole disk</strong>, partition table included ·{" "}
                {d ? formatBytesSi(d.size) : ""}
              </span>
            </label>
            {desc.partitions.map((p) => (
              <div key={p.index}>
                <label className="option">
                  <input
                    type="radio"
                    name="scope"
                    checked={sameScope(scope, { kind: "partition", index: p.index })}
                    onChange={() => setScope({ kind: "partition", index: p.index })}
                  />
                  <span>
                    Partition {p.index}: {p.type_name} · {formatBytesSi(p.length)}
                  </span>
                </label>
                <label className="option">
                  <input
                    type="radio"
                    name="scope"
                    checked={sameScope(scope, { kind: "free_space", index: p.index })}
                    onChange={() => setScope({ kind: "free_space", index: p.index })}
                  />
                  <span>
                    Only the free space of partition {p.index}
                    {sameScope(scope, { kind: "free_space", index: p.index }) && space
                      ? ` · ${space.filesystem} · ${formatBytesSi(space.free_bytes)} free in ${space.free_ranges} range(s)`
                      : ""}
                  </span>
                </label>
              </div>
            ))}
            {scope.kind === "free_space" && (
              <label className="option">
                <input type="checkbox" checked={slack} onChange={(e) => setSlack(e.target.checked)} />
                <span>
                  Also overwrite the slack at the end of every file
                  {slack && space ? ` · ${formatBytesSi(space.slack_bytes)} in ${space.slack_ranges} range(s)` : ""}
                </span>
              </label>
            )}
            {desc.volumes.length > 0 && (
              <p className="muted">
                Mounted on the target: {desc.volumes.map((v) => v.mount).join(", ")}
                .{" "}
                {desc.volumes.every((v) => v.can_dismount)
                  ? "They will be dismounted before the first write."
                  : "Unmount them first."}
              </p>
            )}
          </>
        )}
        {describing && <p className="muted">Reading the volume…</p>}
        {desc.warnings.map((w, i) => (
          <p key={i} className="warn">
            {w}
          </p>
        ))}
        {(freeSpace || filesTarget) && (
          <p className="muted">
            Read{" "}
            <a href={catalogue.article_files} target="_blank" rel="noreferrer">
              Borrado seguro de archivos
            </a>{" "}
            (pcoronaf, 2020) on what a free-space wipe reaches and what it cannot.
          </p>
        )}
      </fieldset>

      <fieldset>
        <legend>Method</legend>
        <select value={method} onChange={(e) => setMethod(e.target.value)}>
          {firmwareOffered && fw && (
            <optgroup label="The device's own sanitize (reaches remapped cells)">
              {fw.supports.map((a) => {
                const info = catalogue.firmware_actions.find((m) => m.id === a);
                const blocked = firmwareBlocked(a);
                return (
                  <option key={`firmware:${a}`} value={`firmware:${a}`} disabled={blocked !== null}>
                    {info?.label ?? a}
                    {blocked ? ` (${blocked})` : ""}
                  </option>
                );
              })}
            </optgroup>
          )}
          <optgroup label="Software overwrite">
            {catalogue.methods.map((m) => (
              <option key={m.id} value={m.id}>
                {m.label}
              </option>
            ))}
          </optgroup>
        </select>
        <p className="muted">{chosen.note}</p>
        {firmwareAction && (
          <p className="warn">{catalogue.firmware_note}</p>
        )}
        {firmwareOffered && fw?.frozen && (
          <p className="warn">Security frozen: {catalogue.frozen_advice}.</p>
        )}
        {firmwareOffered && fw?.note && !fw.frozen && <p className="muted">{fw.note}</p>}
        {!mounted && !filesTarget && scope.kind === "whole_device" && desc.solid_state && !firmwareOffered && (
          <p className="muted">
            {fw?.note ?? "The device's firmware sanitize is not reachable through this platform (USB bridges rarely forward the commands); a software overwrite is a Clear, not a Purge."}
          </p>
        )}
        {chosen.multi_pass && (
          <p className="warn">
            {catalogue.multi_pass_warning} Read{" "}
            <a
              href={catalogue.article_gutmann}
              target="_blank"
              rel="noreferrer"
            >
              El mito de Gutmann
            </a>{" "}
            (pcoronaf, 2014).
          </p>
        )}
        <label className="option">
          <span>Verification after the last pass</span>
          <select
            value={verify}
            onChange={(e) => setVerify(e.target.value as VerifyMode)}
          >
            <option value="sample">
              Read back a sample (every block under 1 GB)
            </option>
            <option value="full">Read back everything</option>
            <option value="none">None</option>
          </select>
        </label>
        <label className="option">
          <span>Then look for recoverable files with the recovery engines</span>
          <select
            value={postScan}
            onChange={(e) => setPostScan(e.target.value as PostScan)}
          >
            <option value="none">No scan</option>
            <option value="quick">Quick scan (filesystem metadata)</option>
            <option value="deep">Deep scan (metadata and carving; takes as long as a recovery scan)</option>
          </select>
        </label>
        {(mounted || filesTarget) && postScan !== "none" && (
          <p className="muted">
            The scan reads the volume directly and needs administrator rights; without them the certificate says the scan did not run.
          </p>
        )}
      </fieldset>

      <fieldset>
        <legend>Certificate (optional)</legend>
        <div className="dest">
          <input
            type="text"
            placeholder="Sanitization certificate (.html, .md or .json); leave empty for none"
            value={certificate}
            onChange={(e) => setCertificate(e.target.value)}
          />
          <button type="button" onClick={pick}>
            Choose…
          </button>
        </div>
        <div className="case-grid">
          <input
            type="text"
            placeholder="Operator"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <input
            type="text"
            placeholder="Organisation"
            value={organisation}
            onChange={(e) => setOrganisation(e.target.value)}
          />
          <input
            type="text"
            placeholder="Asset, case or ticket number"
            value={asset}
            onChange={(e) => setAsset(e.target.value)}
          />
          <input
            type="text"
            placeholder="Notes"
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
          />
        </div>
      </fieldset>

      <div className="danger">
        <p>{catalogue.warning}</p>
        <p>
          Type <strong>{desc.confirmation}</strong>
          {desc.confirmation_short ? (
            <>
              {" "}
              (or its last six characters,{" "}
              <strong>{desc.confirmation_short}</strong>)
            </>
          ) : null}{" "}
          to confirm.
        </p>
        <input
          type="text"
          className="confirm"
          placeholder={desc.confirmation_short ?? desc.confirmation}
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          disabled={countdown !== null}
        />
      </div>
      <div className="actions">
        {countdown === null ? (
          <button
            className="primary erase-button"
            disabled={
              !confirmed || describing || desc.refusals.length > 0 || desc.length === 0
            }
            onClick={() => setCountdown(5)}
          >
            Erase {what}
          </button>
        ) : (
          <>
            <span className="error">Erasing in {countdown} s…</span>
            <button onClick={() => setCountdown(null)}>Cancel</button>
          </>
        )}
      </div>
    </div>
  );
}
