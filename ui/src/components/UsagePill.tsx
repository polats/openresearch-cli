// Usage window on the chat: the active harness's plan windows (percent remaining
// + reset time), surfaced as a compact pill in the composer. Reads the tightest
// window for the pill; the popover lists them all. Data comes from the same
// `getHarnesses` the Harnesses settings tab uses (usage may need a refresh).
import { useEffect, useRef, useState } from "react";
import { RefreshCw } from "lucide-react";
import { getHarnesses, type Harness } from "../api";

function severity(pct: number): "ok" | "warn" | "crit" {
  return pct <= 10 ? "crit" : pct <= 25 ? "warn" : "ok";
}
function sevColor(pct: number): string {
  const s = severity(pct);
  return s === "crit" ? "var(--accent-red)" : s === "warn" ? "var(--accent-amber)" : "var(--accent-green)";
}

/** true when the reset is close enough to render as a countdown ("3h 12m"). */
function isNear(ms: number): boolean {
  const diff = ms - Date.now();
  return diff > 0 && diff < 18 * 3600_000;
}
/** "3h 12m" / "52m" / "now" for near resets; a weekday + clock for far ones. */
function fmtReset(ms?: number): string {
  if (!ms) return "";
  const diff = ms - Date.now();
  if (diff <= 0) return "now";
  const mins = Math.floor(diff / 60000);
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 18) return `${hrs}h ${mins % 60}m`;
  const d = new Date(ms);
  return `${d.toLocaleDateString(undefined, { weekday: "short" })} ${d.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  })}`;
}
/** "resets" clause for the popover: "resets in 3h 12m" vs "resets Mon 11:00 AM". */
function resetPhrase(ms?: number): string {
  if (!ms) return "";
  return `resets ${isNear(ms) ? "in " : ""}${fmtReset(ms)}`;
}
function fmtAgo(ms?: number): string {
  if (!ms) return "";
  const s = Math.floor((Date.now() - ms) / 1000);
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  return `${h}h ago`;
}

function Ring({ pct, color, size = 18 }: { pct: number; color: string; size?: number }) {
  const r = size / 2 - 1.3;
  const c = 2 * Math.PI * r;
  return (
    <svg className="usage-ring" width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden="true">
      <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="var(--border-variant)" strokeWidth="2.6" />
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        stroke={color}
        strokeWidth="2.6"
        strokeLinecap="round"
        strokeDasharray={c}
        strokeDashoffset={c * (1 - Math.max(0, Math.min(100, pct)) / 100)}
        transform={`rotate(-90 ${size / 2} ${size / 2})`}
      />
    </svg>
  );
}

export function UsagePill({ harness }: { harness?: Harness }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [data, setData] = useState<Harness | undefined>(harness);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => setData(harness), [harness]);

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  async function refresh() {
    if (!data) return;
    setBusy(true);
    try {
      const hs = await getHarnesses(true);
      const h = hs.find((x) => x.id === data.id);
      if (h) setData(h);
    } catch {
      /* leave stale data */
    } finally {
      setBusy(false);
    }
  }
  // On first open with no usage yet, fetch it lazily.
  const openWith = () => {
    setOpen((v) => !v);
    if (!open && !data?.usage) void refresh();
  };

  if (!data) return null;

  const usage = data.usage;
  const windows = usage?.windows ?? [];
  const tight = windows.length
    ? windows.reduce((a, b) => (b.remainingPercent < a.remainingPercent ? b : a))
    : null;
  const sev = tight ? severity(tight.remainingPercent) : "ok";

  return (
    <div className="usage-pill-wrap" ref={ref}>
      <button
        className={`usage-pill ${sev}`}
        onClick={openWith}
        title="Usage remaining for this harness"
        aria-haspopup="dialog"
        aria-expanded={open}
      >
        {tight ? (
          <>
            <Ring pct={tight.remainingPercent} color={sevColor(tight.remainingPercent)} />
            <b>{Math.round(tight.remainingPercent)}%</b>
            {tight.resetsAtMs && <span className="usage-reset">{fmtReset(tight.resetsAtMs)}</span>}
          </>
        ) : (
          <span className="usage-ghost">usage</span>
        )}
      </button>
      {open && (
        <div className="usage-pop" role="dialog">
          <div className="usage-pop-h">
            <span className="who">{data.name}</span>
            {usage?.observedAtMs && <span className="obs">seen {fmtAgo(usage.observedAtMs)}</span>}
          </div>
          {windows.length > 0 ? (
            windows.map((w, i) => (
              <div className="usage-wrow" key={i}>
                <div className="usage-wt">
                  <div className="usage-wlabel">{w.label}</div>
                  {w.resetsAtMs && <div className="usage-wsub">{resetPhrase(w.resetsAtMs)}</div>}
                  <div className="usage-bar">
                    <i style={{ width: `${w.remainingPercent}%`, background: sevColor(w.remainingPercent) }} />
                  </div>
                </div>
                <div className="usage-pct" style={{ color: sevColor(w.remainingPercent) }}>
                  {Math.round(w.remainingPercent)}%
                </div>
              </div>
            ))
          ) : (
            <div className="usage-empty">
              {usage?.note ?? (busy ? "Checking usage…" : "No usage limits reported.")}
            </div>
          )}
          <div className="usage-pop-foot">
            <button className="usage-refresh" onClick={refresh} disabled={busy}>
              <RefreshCw size={12} className={busy ? "spin" : ""} /> {busy ? "Refreshing…" : "Refresh"}
            </button>
            {usage?.manageUrl && (
              <a href={usage.manageUrl} target="_blank" rel="noreferrer">
                Manage ↗
              </a>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
