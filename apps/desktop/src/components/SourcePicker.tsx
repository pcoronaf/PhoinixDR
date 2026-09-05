import type { DeviceInfo } from "../types";
import { formatBytesSi } from "../lib/format";

interface Props {
  devices: DeviceInfo[];
  removableOnly: boolean;
  loading: boolean;
  error: string | null;
  onChoose: (d: DeviceInfo) => void;
  onBack: () => void;
  onRefresh: () => void;
}

export function SourcePicker({ devices, removableOnly, loading, error, onChoose, onBack, onRefresh }: Props) {
  const shown = devices.filter((d) => (removableOnly ? d.removable !== false : true));
  const blocked = shown.some((d) => !d.accessible);
  return (
    <div className="panel">
      <div className="row-between">
        <h2>{removableOnly ? "Removable devices" : "Physical disks"}</h2>
        <div>
          <button className="link" onClick={onRefresh} disabled={loading}>Refresh</button>
          <button className="link" onClick={onBack}>Back</button>
        </div>
      </div>
      {loading && <p className="muted">Enumerating devices…</p>}
      {error && <p className="error">{error}</p>}
      {!loading && shown.length === 0 && <p className="muted">No devices found.</p>}
      {blocked && (
        <p className="warn">
          Some devices are not accessible from this process: reading a disk or USB stick directly requires administrator rights.
          Close PhoinixDR and start it again with <em>Run as administrator</em> (Windows) or <code>sudo</code> (Linux). Disk images do not need this.
        </p>
      )}
      <div className="cards">
        {shown.map((d) => (
          <button key={d.id} className="card" disabled={!d.accessible} onClick={() => onChoose(d)}>
            <strong>{d.display_name}</strong>
            <span>{formatBytesSi(d.size)}{d.bus ? ` · ${d.bus.toUpperCase()}` : ""}{d.rotational === false ? " · SSD" : d.rotational ? " · HDD" : ""}{d.removable ? " · removable" : ""}</span>
            <span className="mono muted">{d.path}</span>
            {!d.accessible && <span className="warn">Not accessible from this process: run PhoinixDR as administrator to scan this device</span>}
          </button>
        ))}
      </div>
    </div>
  );
}
