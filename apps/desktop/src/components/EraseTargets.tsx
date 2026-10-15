import type { Api } from "../api";
import type { AppInfo, EraseCatalogue, EraseTargetInfo, MountedVolumeInfo } from "../types";
import { formatBytesSi } from "../lib/format";
import { refusalText } from "../lib/erase";
import { ElevateNotice } from "./ElevateNotice";

interface Props {
  api: Api;
  info: AppInfo | null;
  catalogue: EraseCatalogue | null;
  targets: EraseTargetInfo[];
  volumes: MountedVolumeInfo[];
  loading: boolean;
  error: string | null;
  onChoose: (t: EraseTargetInfo) => void;
  onChooseVolume: (v: MountedVolumeInfo) => void;
  onChooseFiles: (paths: string[]) => void;
  onBack: () => void;
  onRefresh: () => void;
}

/** Whether a listing error means the device needs administrator rights. */
function needsElevation(t: EraseTargetInfo): boolean {
  return t.error !== null && /permission|elevated|denied|access/i.test(t.error);
}

/** The disks that can be erased, with the reasons the others cannot, and
 *  the mounted volumes whose free space can be wiped through the
 *  filesystem. */
export function EraseTargets({
  api,
  info,
  catalogue,
  targets,
  volumes,
  loading,
  error,
  onChoose,
  onChooseVolume,
  onChooseFiles,
  onBack,
  onRefresh,
}: Props) {
  const blocked = targets.some(needsElevation);
  const pickFiles = async (): Promise<void> => {
    const paths = await api.pickEraseFiles();
    if (paths.length > 0) onChooseFiles(paths);
  };
  const pickFolder = async (): Promise<void> => {
    const p = await api.pickEraseFolder();
    if (p) onChooseFiles([p]);
  };
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
      {!loading && targets.length === 0 && volumes.length === 0 && (
        <p className="muted">
          No disks found. Device access needs administrator rights.
        </p>
      )}
      {blocked && <ElevateNotice api={api} info={info} context="devices" />}
      <div className="cards">
        {targets.map((t) => {
          const blockedTarget = t.refusals.length > 0 || t.error !== null;
          return (
            <button
              key={t.device.id}
              className={`card ${blockedTarget ? "" : "erase-ok"}`}
              disabled={blockedTarget}
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
        A disk's page offers the whole disk, one partition, or the free
        space of a partition (with the slack of its files if you wish). Disk
        images: use the file or free-space target on the command line.
      </p>

      <h2>Wipe the free space of a volume in use</h2>
      <p className="muted">
        Fills the free space of a mounted volume through the filesystem,
        then removes the fill files. Needs no administrator rights and the
        volume stays usable; live files are never touched.
      </p>
      {!loading && volumes.length === 0 && (
        <p className="muted">No writable volumes found.</p>
      )}
      <div className="cards">
        {volumes.map((v) => (
          <button
            key={v.node}
            className={`card ${v.read_only ? "" : "erase-ok"}`}
            disabled={v.read_only}
            onClick={() => onChooseVolume(v)}
          >
            <strong>
              {v.mount}
              {v.label ? ` ${v.label}` : ""}
            </strong>
            <span>
              {v.filesystem} · {formatBytesSi(v.free)} free of{" "}
              {formatBytesSi(v.total)}
              {v.system ? " · system volume" : ""}
              {v.read_only ? " · read-only" : ""}
            </span>
            <span className="mono muted">
              {v.node}
              {v.disk ? ` on ${v.disk}` : ""}
            </span>
          </button>
        ))}
      </div>

      <h2>Erase files and folders</h2>
      <p className="muted">
        Overwrites the content of the files you choose through the
        filesystem (named streams and slack included), then truncates,
        renames and deletes them. What this cannot reach is stated on the
        next page; follow with a free-space wipe of the volume.
      </p>
      <div className="cards">
        <button className="card erase-ok" onClick={pickFiles}>
          <strong>Choose files…</strong>
          <span>One or more files</span>
        </button>
        <button className="card erase-ok" onClick={pickFolder}>
          <strong>Choose a folder…</strong>
          <span>Every file under it, then the folder itself</span>
        </button>
      </div>
    </div>
  );
}
