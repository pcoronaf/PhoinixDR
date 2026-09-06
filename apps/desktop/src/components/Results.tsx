import { useEffect, useMemo, useState } from "react";
import type { Api } from "../api";
import type { CandidateSummary, EngineLogLine, Preview, RecoveryCandidate, ScanRequest, SessionSummary } from "../types";
import { applyFilters, buildTree, CATEGORY_LABEL, CATEGORY_ORDER, DEFAULT_FILTERS, sortRows, typeOptions, type Filters, type SortKey, type TreeNode } from "../lib/filters";
import { formatBytes, formatDate, fsLabel } from "../lib/format";
import { HealthBadge } from "./HealthBadge";
import { CommandLine, CopyButton, logText } from "./Advanced";
import { explainCommand, recoverCommand } from "../lib/cli";

interface Props {
  api: Api;
  session: SessionSummary;
  rows: CandidateSummary[];
  advanced: boolean;
  /** The request that produced the session, when known (for the command lines). */
  request?: ScanRequest | null;
  /** The engine log of the scan, for the Copy scan log action. */
  log?: EngineLogLine[];
  onRecover: (ids: string[]) => void;
  onNewScan: () => void;
}

export function Results({ api, session, rows, advanced, request = null, log = [], onRecover, onNewScan }: Props) {
  const [filters, setFilters] = useState<Filters>(DEFAULT_FILTERS);
  const [sort, setSort] = useState<{ key: SortKey; asc: boolean }>({ key: "likelihood", asc: false });
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [focus, setFocus] = useState<string | null>(null);

  const filtered = useMemo(() => sortRows(applyFilters(rows, filters), sort.key, sort.asc), [rows, filters, sort]);
  const tree = useMemo(() => buildTree(rows), [rows]);
  const types = useMemo(() => typeOptions(rows), [rows]);
  const focused = rows.find((r) => r.id === focus) ?? null;

  const toggle = (id: string): void =>
    setSelected((cur) => {
      const next = new Set(cur);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const selectAllFiltered = (): void => setSelected(new Set(filtered.map((r) => r.id)));
  const clear = (): void => setSelected(new Set());
  const sortBy = (key: SortKey): void => setSort((s) => ({ key, asc: s.key === key ? !s.asc : key === "name" || key === "path" }));

  return (
    <div className="results">
      <aside className="tree">
        <div className="tree-head">
          <strong>{session.source.split(/[\\/]/).pop()}</strong>
          <span className="muted">{fsLabel(session.filesystem)} · {session.mode === "deep" ? "deep" : "quick"} scan{session.complete ? "" : " (partial)"}{session.partition === null && session.source ? "" : ""}</span>
        </div>
        <Tree node={tree} depth={0} active={filters.folder} onPick={(key) => setFilters((f) => ({ ...f, folder: f.folder === key ? null : key }))} />
        {session.carving && (
          <div className="carve-stats muted">
            Deep scan: {session.carving.hits} hits, {session.carving.merged_into_metadata} merged into filesystem records, {session.carved} carved files listed.
          </div>
        )}
      </aside>
      <section className="table-area">
        <FilterBar filters={filters} setFilters={setFilters} types={types} total={rows.length} shown={filtered.length} />
        <div className="table-scroll">
          <table className="candidates">
            <thead>
              <tr>
                <th className="check"><input type="checkbox" checked={filtered.length > 0 && filtered.every((r) => selected.has(r.id))} onChange={(e) => (e.target.checked ? selectAllFiltered() : clear())} /></th>
                <th onClick={() => sortBy("name")}>Name</th>
                <th onClick={() => sortBy("likelihood")}>Recovery</th>
                <th onClick={() => sortBy("size")} className="num">Size</th>
                <th>Type</th>
                <th onClick={() => sortBy("modified")}>Modified</th>
                <th onClick={() => sortBy("path")}>Original location</th>
                {advanced && <th>Ref</th>}
              </tr>
            </thead>
            <tbody>
              {filtered.map((r) => (
                <tr key={r.id} className={`${focus === r.id ? "focus" : ""} ${selected.has(r.id) ? "selected" : ""}`} onClick={() => setFocus(r.id)}>
                  <td className="check" onClick={(e) => e.stopPropagation()}><input type="checkbox" checked={selected.has(r.id)} onChange={() => toggle(r.id)} /></td>
                  <td className="name">{r.name}{r.source === "file_carving" && <span className="tag">carved</span>}{r.source === "journal" && <span className="tag">journal</span>}</td>
                  <td><HealthBadge category={r.category} likelihood={r.likelihood} confidence={r.confidence} /></td>
                  <td className="num">{formatBytes(r.size)}</td>
                  <td>{r.type_name ?? <span className="muted">–</span>}</td>
                  <td>{formatDate(r.modified)}</td>
                  <td className="mono path">{r.path ?? <span className="muted">unknown (carved)</span>}{r.path_uncertain && r.path ? " (uncertain)" : ""}</td>
                  {advanced && <td className="mono">{r.reference}</td>}
                </tr>
              ))}
            </tbody>
          </table>
          {filtered.length === 0 && <p className="muted center">No candidates match the current filters.</p>}
        </div>
        <div className="actionbar">
          <span>{selected.size} selected · {filtered.length} shown of {rows.length}</span>
          <div>
            {advanced && log.length > 0 && <CopyButton text={logText(log)} label="Copy scan log" />}
            <button className="link" onClick={onNewScan}>New scan</button>
            <button className="primary" disabled={selected.size === 0} onClick={() => onRecover([...selected])}>Recover {selected.size > 0 ? `${selected.size} file${selected.size === 1 ? "" : "s"}` : ""}…</button>
          </div>
        </div>
      </section>
      <aside className="detail">
        {focused ? <Detail api={api} row={focused} advanced={advanced} session={session} request={request} /> : <p className="muted center">Select a file to see why PhoinixDR rates it this way, and a preview.</p>}
      </aside>
    </div>
  );
}

function Tree({ node, depth, active, onPick }: { node: TreeNode; depth: number; active: string | null; onPick: (key: string) => void }) {
  const [open, setOpen] = useState(depth < 2);
  const isActive = active === node.key || (active === null && node.key === "");
  return (
    <div className="tree-node">
      <div className={`tree-row ${isActive ? "active" : ""}`} style={{ paddingLeft: `${depth * 14 + 6}px` }}>
        {node.children.length > 0 ? (
          <button className="twisty" onClick={() => setOpen((o) => !o)} aria-label={open ? "collapse" : "expand"}>{open ? "▾" : "▸"}</button>
        ) : (
          <span className="twisty" />
        )}
        <button className="tree-label" onClick={() => onPick(node.key)}>{node.name} <span className="muted">{node.count}</span></button>
      </div>
      {open && node.children.map((c) => <Tree key={c.key} node={c} depth={depth + 1} active={active} onPick={onPick} />)}
    </div>
  );
}

function FilterBar({ filters, setFilters, types, total, shown }: { filters: Filters; setFilters: (f: (cur: Filters) => Filters) => void; types: { id: string; name: string; count: number }[]; total: number; shown: number }) {
  return (
    <div className="filters">
      <input type="search" placeholder="Search file name or path" value={filters.search} onChange={(e) => setFilters((f) => ({ ...f, search: e.target.value }))} />
      <select value={filters.minCategory ?? ""} onChange={(e) => setFilters((f) => ({ ...f, minCategory: (e.target.value || null) as Filters["minCategory"] }))}>
        <option value="">Any recovery health</option>
        {CATEGORY_ORDER.filter((c) => c !== "Unknown" && c !== "Unrecoverable").map((c) => (
          <option key={c} value={c}>{CATEGORY_LABEL[c]} or better</option>
        ))}
      </select>
      <select value={filters.source} onChange={(e) => setFilters((f) => ({ ...f, source: e.target.value as Filters["source"] }))}>
        <option value="all">Deleted and carved</option>
        <option value="metadata">Deleted files only</option>
        <option value="carved">Carved files only</option>
      </select>
      <select value={filters.types[0] ?? ""} onChange={(e) => setFilters((f) => ({ ...f, types: e.target.value ? [e.target.value] : [] }))}>
        <option value="">Any type</option>
        {types.map((t) => (
          <option key={t.id} value={t.id}>{t.name} ({t.count})</option>
        ))}
      </select>
      {(filters.search || filters.minCategory || filters.source !== "all" || filters.types.length > 0 || filters.folder !== null) && (
        <button className="link" onClick={() => setFilters(() => DEFAULT_FILTERS)}>Clear filters</button>
      )}
      <span className="muted">{shown} of {total}</span>
    </div>
  );
}

function Detail({ api, row, advanced, session, request }: { api: Api; row: CandidateSummary; advanced: boolean; session: SessionSummary; request: ScanRequest | null }) {
  const sameScan = request !== null && request.source === session.source;
  const volume = sameScan ? request.volume ?? null : null;
  const partition = sameScan ? request.partition : session.partition;
  const [tab, setTab] = useState<"evidence" | "preview">("evidence");
  const [detail, setDetail] = useState<RecoveryCandidate | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setDetail(null);
    setPreview(null);
    setError(null);
    api.candidateDetail(row.id).then((d) => alive && setDetail(d)).catch((e: unknown) => alive && setError(String(e)));
    return () => {
      alive = false;
    };
  }, [api, row.id]);

  useEffect(() => {
    if (tab !== "preview" || preview) return;
    let alive = true;
    api.preview(row.id).then((p) => alive && setPreview(p)).catch((e: unknown) => alive && setPreview({ kind: "unavailable", reason: String(e) }));
    return () => {
      alive = false;
    };
  }, [api, row.id, tab, preview]);

  return (
    <div className="detail-body">
      <h3 className="name" title={row.name}>{row.name}</h3>
      <HealthBadge category={row.category} likelihood={row.likelihood} />
      <p className="muted">Assessment confidence {row.confidence}% · {formatBytes(row.size)}{row.type_name ? ` · ${row.type_name}` : ""}</p>
      <div className="tabs">
        <button className={tab === "evidence" ? "on" : ""} onClick={() => setTab("evidence")}>Evidence</button>
        <button className={tab === "preview" ? "on" : ""} onClick={() => setTab("preview")}>Preview</button>
      </div>
      {error && <p className="error">{error}</p>}
      {tab === "evidence" && detail && (
        <div className="evidence">
          <ul className="reasons">
            {detail.health.reasons.map((r, i) => (
              <li key={i} className={r.positive ? "good" : "bad"}>{r.positive ? "✓" : "⚠"} {r.text}</li>
            ))}
          </ul>
          {detail.evidence.content.validation && (
            <details open={advanced}>
              <summary>Structure validation: {detail.evidence.content.validation.status}</summary>
              <ul className="checks">
                {detail.evidence.content.validation.checks.map((c, i) => (
                  <li key={i} className={c.passed ? "good" : "bad"}>{c.passed ? "✓" : "✗"} {c.name}: {c.detail}</li>
                ))}
              </ul>
            </details>
          )}
          {advanced && (
            <dl className="advanced">
              <dt>Object</dt><dd className="mono">{JSON.stringify(detail.filesystem_object)}</dd>
              <dt>Extents</dt><dd>{detail.evidence.extents.resident ? "resident" : `${detail.evidence.extents.extent_count} fragment(s), ${detail.evidence.extents.total_clusters ?? "?"} clusters`}{detail.evidence.extents.complete ? "" : " (incomplete)"}{detail.evidence.extents.chain_known ? "" : ", layout inferred"}{detail.evidence.extents.start_inferred ? ", start inferred" : ""}</dd>
              <dt>Allocation</dt><dd>{detail.evidence.allocation.map_available ? `${detail.evidence.allocation.clusters_free} free, ${detail.evidence.allocation.clusters_allocated} allocated, ${detail.evidence.allocation.clusters_unknown} unknown of ${detail.evidence.allocation.clusters_total}` : "no allocation map"}</dd>
              <dt>Content</dt><dd>{detail.evidence.content.bytes_examined} bytes examined{detail.evidence.content.zero_block_ratio !== null ? `, ${Math.round(detail.evidence.content.zero_block_ratio * 100)}% zero-filled samples` : ""}</dd>
              <dt>Timestamps</dt><dd>created {formatDate(detail.timestamps.created_iso)} · modified {formatDate(detail.timestamps.modified_iso)} · accessed {formatDate(detail.timestamps.accessed_iso)}</dd>
              <dt>Storage</dt><dd>{detail.evidence.storage.device_kind}{detail.evidence.storage.rotational === false ? ", solid state (TRIM state unknown)" : detail.evidence.storage.rotational ? ", rotational" : ""}</dd>
            </dl>
          )}
          {advanced && (
            <CommandLine
              title="Command line"
              commands={[explainCommand(session.source, partition, volume, row.reference), recoverCommand(session.source, partition, volume, [row.reference], null)]}
            />
          )}
        </div>
      )}
      {tab === "preview" && (
        <div className="preview">
          {!preview && <p className="muted">Loading preview…</p>}
          {preview?.kind === "image" && <img alt={row.name} src={`data:${preview.mime};base64,${preview.base64}`} />}
          {preview?.kind === "text" && <pre>{preview.text}{preview.truncated ? "\n…" : ""}</pre>}
          {preview?.kind === "hex" && <pre className="hex">{preview.dump}</pre>}
          {preview?.kind === "unavailable" && <p className="muted">No preview: {preview.reason}</p>}
        </div>
      )}
    </div>
  );
}
