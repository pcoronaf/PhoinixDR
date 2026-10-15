import { useEffect, useState } from "react";
import type { Api } from "../api";
import type {
  EraseCatalogue,
  EraseRequest,
  EraseTarget,
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

function text(s: string): string | null {
  const t = s.trim();
  return t.length > 0 ? t : null;
}

/** Target, method, operator, confirmation and the countdown. */
export function EraseSetup({
  api,
  catalogue,
  initial,
  onStart,
  onBack,
}: Props) {
  const [desc, setDesc] = useState<TargetDescription>(initial);
  const [scope, setScope] = useState<"whole_device" | number>(
    initial.target.kind === "partition" ? initial.target.index : "whole_device",
  );
  const [method, setMethod] = useState("zero");
  const [verify, setVerify] = useState<VerifyMode>("sample");
  const [certificate, setCertificate] = useState("");
  const [name, setName] = useState("");
  const [organisation, setOrganisation] = useState("");
  const [asset, setAsset] = useState("");
  const [notes, setNotes] = useState("");
  const [typed, setTyped] = useState("");
  const [countdown, setCountdown] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const chosen =
    catalogue.methods.find((m) => m.id === method) ?? catalogue.methods[0]!;
  const confirmed =
    typed.trim().length > 0 &&
    (typed.trim().toLowerCase() === desc.confirmation.toLowerCase() ||
      (desc.confirmation_short !== null &&
        typed.trim().toLowerCase() === desc.confirmation_short.toLowerCase()));

  // Re-describe when the scope changes (whole device or one partition).
  useEffect(() => {
    const target: EraseTarget =
      scope === "whole_device"
        ? { kind: "whole_device", path: initial.path }
        : { kind: "partition", path: initial.path, index: scope };
    let alive = true;
    api
      .describeEraseTarget(target)
      .then((d) => alive && setDesc(d))
      .catch((e: unknown) => alive && setError(String(e)));
    return () => {
      alive = false;
    };
  }, [api, initial.path, scope]);

  // Countdown.
  useEffect(() => {
    if (countdown === null) return;
    if (countdown === 0) {
      const request: EraseRequest = {
        target: desc.target,
        method: { id: method },
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
    verify,
    name,
    organisation,
    asset,
    notes,
    certificate,
    typed,
    onStart,
  ]);

  const pick = async (): Promise<void> => {
    const p = await api.pickCertificateFile();
    if (p) setCertificate(p);
  };
  const d = desc.device;

  return (
    <div className="panel erase">
      <div className="row-between">
        <h2>Erase {d ? d.display_name : desc.path}</h2>
        <button className="link" onClick={onBack} disabled={countdown !== null}>
          Back
        </button>
      </div>
      <p className="mono muted">
        {desc.path}
        {d?.serial ? ` · S/N ${d.serial}` : ""} · {formatBytesSi(desc.length)}
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
        <label className="option">
          <input
            type="radio"
            name="scope"
            checked={scope === "whole_device"}
            onChange={() => setScope("whole_device")}
          />
          <span>
            <strong>The whole disk</strong>, partition table included ·{" "}
            {d ? formatBytesSi(d.size) : ""}
          </span>
        </label>
        {desc.partitions.map((p) => (
          <label key={p.index} className="option">
            <input
              type="radio"
              name="scope"
              checked={scope === p.index}
              onChange={() => setScope(p.index)}
            />
            <span>
              Partition {p.index}: {p.type_name} · {formatBytesSi(p.length)}
            </span>
          </label>
        ))}
        {desc.volumes.length > 0 && (
          <p className="muted">
            Mounted on the target: {desc.volumes.map((v) => v.mount).join(", ")}
            .{" "}
            {desc.volumes.every((v) => v.can_dismount)
              ? "They will be dismounted before the first write."
              : "Unmount them first."}
          </p>
        )}
      </fieldset>

      <fieldset>
        <legend>Method</legend>
        <select value={method} onChange={(e) => setMethod(e.target.value)}>
          {catalogue.methods.map((m) => (
            <option key={m.id} value={m.id}>
              {m.label}
            </option>
          ))}
        </select>
        <p className="muted">{chosen.note}</p>
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
              !confirmed || desc.refusals.length > 0 || desc.length === 0
            }
            onClick={() => setCountdown(5)}
          >
            Erase{" "}
            {scope === "whole_device" ? "the whole disk" : `partition ${scope}`}
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
