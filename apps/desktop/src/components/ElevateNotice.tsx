import { useState } from "react";
import type { Api } from "../api";
import type { AppInfo } from "../types";

interface Props {
  api: Api;
  info: AppInfo | null;
  /** Where the notice is shown: the home page (no device can be listed) or the device picker (some devices are not accessible). */
  context: "home" | "devices";
}

/**
 * Explains that reading devices needs administrator rights and offers to
 * restart PhoinixDR elevated through the operating system's own prompt.
 */
export function ElevateNotice({ api, info, context }: Props) {
  const [busy, setBusy] = useState(false);
  const [exiting, setExiting] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const platform = info?.platform ?? "windows";
  const label = platform === "windows" ? "Restart as administrator" : "Restart with administrator rights";
  const manual =
    platform === "windows"
      ? "Or close PhoinixDR, right-click the executable and choose Run as administrator."
      : "Or close PhoinixDR and start it again with sudo.";

  if (info?.elevated) {
    return (
      <p className="warn">
        {context === "home"
          ? "Devices cannot be listed even though PhoinixDR runs with administrator rights; the system may block raw disk access (policy, antivirus)."
          : "Some devices cannot be opened even though PhoinixDR runs with administrator rights; they may be in use by another program or protected by the system."}{" "}
        Disk images do not need device access.
      </p>
    );
  }

  const relaunch = async () => {
    setBusy(true);
    setError(null);
    try {
      const exits = await api.relaunchElevated();
      if (exits) setExiting(true);
      else setPending(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="warn elevate">
      <p>
        {context === "home"
          ? "Devices cannot be listed: reading a disk or USB stick directly requires administrator rights."
          : "Some devices are not accessible from this process: reading a disk or USB stick directly requires administrator rights."}{" "}
        Disk images do not need them.
      </p>
      {exiting ? (
        <p>Starting PhoinixDR with administrator rights; this window closes now.</p>
      ) : pending ? (
        <p>A password prompt has opened. When the new PhoinixDR window appears, close this one.</p>
      ) : (
        <p className="elevate-actions">
          <button className="primary" onClick={relaunch} disabled={busy}>{busy ? "Waiting for the system prompt…" : label}</button>
          <span className="muted">{manual}</span>
        </p>
      )}
      {error && <p className="error">{error}</p>}
    </div>
  );
}
