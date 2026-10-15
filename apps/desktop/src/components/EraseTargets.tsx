import type { EraseCatalogue, EraseTargetInfo } from "../types";
import { formatBytesSi } from "../lib/format";
import { refusalText } from "../lib/erase";

interface Props {
  catalogue: EraseCatalogue | null;
  targets: EraseTargetInfo[];
  loading: boolean;
  error: string | null;
  onChoose: (t: EraseTargetInfo) => void;
  onBack: () => void;
  onRefresh: () => void;
}

/** The disks that can be erased, with the reasons the others cannot. */
export function EraseTargets({
  catalogue,
  targets,
  loading,
  error,
  onChoose,
  onBack,
  onRefresh,
}: Props) {
  return (
    <div className="panel erase">
      <div className="row-between">
        <h2>Erase a disk</h2>
        <div>
          <button className="link" onClick={onRefresh} disabled={loading}>
            Refresh
          </button>
          <button className="link" onClick={onBack}>
            Back
          </button>
        </div>
      </div>
      <p className="danger">
        {catalogue?.warning ?? "Erasing is irreversible."}
      </p>
      {loading && <p className="muted">Enumerating disks…</p>}
      {error && <p className="error">{error}</p>}
      {!loading && targets.length === 0 && (
        <p className="muted">
          No disks found. Device access needs administrator rights.
        </p>
      )}
      <div className="cards">
        {targets.map((t) => {
          const blocked = t.refusals.length > 0 || t.error !== null;
          return (
            <button
              key={t.device.id}
              className={`card ${blocked ? "" : "erase-ok"}`}
              disabled={blocked}
              onClick={() => onChoose(t)}
            >
              <strong>{t.device.display_name}</strong>
              <span>
                {formatBytesSi(t.device.size)}
                {t.device.serial
                  ? ` · S/N ${t.device.serial}`
                  : " · no serial number"}
                {t.device.rotational === false
                  ? " · SSD"
                  : t.device.rotational
                    ? " · HDD"
                    : ""}
                {t.device.removable ? " · removable" : ""}
              </span>
              <span className="mono muted">{t.device.path}</span>
              {t.description && t.description.volumes.length > 0 && (
                <span className="muted">
                  Mounted:{" "}
                  {t.description.volumes.map((v) => v.mount).join(", ")}
                </span>
              )}
              {t.error && <span className="warn">{t.error}</span>}
              {t.refusals.map((r, i) => (
                <span key={i} className="warn">
                  Cannot erase: {refusalText(r)}
                </span>
              ))}
            </button>
          );
        })}
      </div>
      <p className="muted">
        Partitions, free space and files are erased from a disk's page. Disk
        images: use the file target on the command line for now.
      </p>
    </div>
  );
}
