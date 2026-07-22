import { ChevronDown, CircleStop, ExternalLink, NotebookPen, RotateCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
  beatPlaySession,
  cancelRun,
  endPlaySession,
  fmtDuration,
  getCommitDiff,
  getWorkingTree,
  listExperimentCommits,
  listRunArtifacts,
  playUrl,
  runArtifactUrl,
  setExperimentPlayEntry,
  setExperimentVerdict,
  setRunVerdict,
  startPlayBuild,
  timeAgo,
  type CommitInfo,
  type DiffPayload,
  type Experiment,
  type Project,
  type Run,
  type RunArtifact,
  type Verdict,
  type WorkingTree,
} from "../api";
import { BranchPill } from "./BranchPill";
import { GitDiff, TruncatedDiffNotice } from "./GitDiff";
import { LogTerminal } from "./LogTerminal";
import { StatusBadge } from "./StatusBadge";

const UNCOMMITTED = "__uncommitted__";

interface DiffState {
  loading: boolean;
  error?: string;
  payload?: DiffPayload;
}

function DiffView({ state }: { state: DiffState | undefined }) {
  if (!state || state.loading) return <div className="changes-note">Loading diff…</div>;
  if (state.error) return <div className="error">{state.error}</div>;
  if (!state.payload) return <div className="diff-empty">No changes.</div>;
  if (state.payload.truncated) {
    return (
      <TruncatedDiffNotice
        bytesRead={state.payload.bytesRead}
        byteLimit={state.payload.byteLimit}
      />
    );
  }
  return <GitDiff diff={state.payload.diff} />;
}

export type ExperimentView = "terminal" | "changes" | "play";

/** The playable, embedded like any other experiment tab. Opening it kicks
 *  the (idempotent) build; the iframe's holding page self-refreshes until
 *  the build lands, so no client-side polling is needed. */
function PlayView({ experiment }: { experiment: Experiment }) {
  const [entry, setEntry] = useState(experiment.playEntry ?? "");
  const [reloadKey, setReloadKey] = useState(0);
  useEffect(() => setEntry(experiment.playEntry ?? ""), [experiment.playEntry]);
  // Kick the build on open and on manual rebuild; reload the frame either way.
  useEffect(() => {
    startPlayBuild(experiment.id).catch((err) => console.error("play-build:", err));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [experiment.id, reloadKey]);

  // Session runs: beat every 10s while the tab is mounted — the first beat
  // after the build lands starts the session (earlier ones error and are
  // ignored), so build wait never counts as play time. Giving a verdict ends
  // the session and the verdict rides the session run; the server-side
  // reaper covers a browser that just vanishes.
  const [session, setSession] = useState<Run | null>(null);
  const [, setTick] = useState(0);
  useEffect(() => {
    let stopped = false;
    const beat = () =>
      beatPlaySession(experiment.id)
        .then((r) => {
          if (!stopped) setSession(r);
        })
        .catch(() => {});
    beat();
    const beats = setInterval(beat, 10_000);
    const clock = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => {
      stopped = true;
      clearInterval(beats);
      clearInterval(clock);
    };
  }, [experiment.id, reloadKey]);
  const endWithVerdict = (v: Verdict | null, notes: string) => {
    endPlaySession(experiment.id, v, notes)
      .then(() => setSession(null))
      .catch((err) => console.error("play-session:", err));
  };

  const src = playUrl({ id: experiment.id, playEntry: experiment.playEntry });
  const saveEntry = () => {
    const next = entry.trim() || null;
    if ((experiment.playEntry ?? null) === next) return;
    setExperimentPlayEntry(experiment.id, next)
      .then(() => setReloadKey((k) => k + 1))
      .catch((err) => console.error("play-entry:", err));
  };
  return (
    <div className="play-view">
      <div className="play-bar">
        <span className="ctl-label">Entry</span>
        <input
          className="input sm play-entry-input"
          placeholder="index.html (e.g. gambit-slots.html?x=1)"
          value={entry}
          onChange={(e) => setEntry(e.target.value)}
          onBlur={saveEntry}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          }}
        />
        <span style={{ flex: 1 }} />
        {session && (
          <>
            <span className="session-clock" title={`Session started ${timeAgo(session.createdAt)}`}>
              <span className="dot" />
              {fmtDuration(Date.now() - session.createdAt)}
            </span>
            <VerdictChips
              verdict={session.verdict ?? null}
              notes={session.verdictNotes}
              onSave={endWithVerdict}
            />
          </>
        )}
        <button
          className="btn sm"
          title="Rebuild from the branch head and reload"
          onClick={() => setReloadKey((k) => k + 1)}
        >
          <RotateCw size={12} /> Rebuild
        </button>
        <a
          className="icon-btn"
          href={src}
          target="_blank"
          rel="noreferrer"
          title="Open in a browser tab"
          aria-label="Open in a browser tab"
        >
          <ExternalLink size={14} />
        </a>
      </div>
      <iframe key={reloadKey} className="play-frame" src={src} title="Playable build" />
    </div>
  );
}

const VERDICTS: Verdict[] = ["keep", "kill", "iterate"];

/** keep / kill / iterate chips + a notes toggle. Clicking the active chip
 *  clears the verdict; notes save on blur. Optimistic — the SSE diff brings
 *  the canonical state ~500ms later. */
function VerdictChips({
  verdict,
  notes,
  onSave,
  label,
}: {
  verdict: Verdict | null | undefined;
  notes: string | null | undefined;
  onSave: (verdict: Verdict | null, notes: string) => void;
  label?: string;
}) {
  const [current, setCurrent] = useState<Verdict | null>(verdict ?? null);
  const [text, setText] = useState(notes ?? "");
  const [notesOpen, setNotesOpen] = useState(false);
  useEffect(() => setCurrent(verdict ?? null), [verdict]);
  useEffect(() => setText(notes ?? ""), [notes]);

  const pick = (v: Verdict) => {
    const next = current === v ? null : v;
    setCurrent(next);
    onSave(next, text);
  };
  return (
    <span className="verdict-control">
      {label && <span className="verdict-label">{label}</span>}
      {VERDICTS.map((v) => (
        <button
          key={v}
          className={`verdict-chip ${v} ${current === v ? "active" : ""}`}
          title={current === v ? `Clear ${v}` : v}
          onClick={() => pick(v)}
        >
          {v}
        </button>
      ))}
      <button
        className={`icon-btn ${notesOpen || text ? "active" : ""}`}
        title={text ? `Notes: ${text}` : "Add notes"}
        aria-label="Verdict notes"
        onClick={() => setNotesOpen((v) => !v)}
      >
        <NotebookPen size={13} />
      </button>
      {notesOpen && (
        <input
          className="input sm verdict-notes"
          placeholder="Why? (saved on blur)"
          value={text}
          autoFocus
          onChange={(e) => setText(e.target.value)}
          onBlur={() => {
            setNotesOpen(false);
            onSave(current, text);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          }}
        />
      )}
    </span>
  );
}

/** Format a metric value compactly (3 significant digits for floats). */
function fmtMetric(v: unknown): string {
  if (typeof v === "number") {
    return Number.isInteger(v) ? String(v) : v.toPrecision(3);
  }
  return String(v);
}

/** Metric chips for a run's `aggregate`, each with a delta vs the parent
 *  experiment's latest sim aggregate when one exists. */
function MetricChips({
  aggregate,
  parentAggregate,
}: {
  aggregate: Record<string, unknown>;
  parentAggregate: Record<string, unknown> | null;
}) {
  return (
    <div className="metric-chips">
      {Object.entries(aggregate).map(([key, value]) => {
        const parent = parentAggregate?.[key];
        const delta =
          typeof value === "number" && typeof parent === "number" ? value - parent : null;
        return (
          <span key={key} className="metric-chip" title={parent != null ? `parent: ${fmtMetric(parent)}` : undefined}>
            <span className="metric-key">{key}</span>
            <span className="metric-value">{fmtMetric(value)}</span>
            {delta !== null && delta !== 0 && (
              <span className={`metric-delta ${delta > 0 ? "up" : "down"}`}>
                {delta > 0 ? "▲" : "▼"}
                {fmtMetric(Math.abs(delta))}
              </span>
            )}
          </span>
        );
      })}
    </div>
  );
}

/** The run's media/output gallery ($ORX_ARTIFACTS_DIR). Images render
 *  inline; everything else is a file chip. All entries open raw in a tab. */
function ArtifactGallery({ runId, terminal }: { runId: string; terminal: boolean }) {
  const [artifacts, setArtifacts] = useState<RunArtifact[] | null>(null);
  useEffect(() => {
    listRunArtifacts(runId)
      .then(setArtifacts)
      .catch(() => setArtifacts([]));
    // Refetch when the run reaches a terminal state — most artifacts land at
    // the end of a run.
  }, [runId, terminal]);
  if (!artifacts || artifacts.length === 0) return null;
  const images = artifacts.filter((a) => a.contentType.startsWith("image/"));
  const rest = artifacts.filter((a) => !a.contentType.startsWith("image/"));
  return (
    <div className="artifact-gallery">
      {images.length > 0 && (
        <div className="artifact-grid">
          {images.map((a) => (
            <a
              key={a.path}
              href={runArtifactUrl(runId, a.path)}
              target="_blank"
              rel="noreferrer"
              title={a.path}
            >
              <img src={runArtifactUrl(runId, a.path)} alt={a.path} loading="lazy" />
            </a>
          ))}
        </div>
      )}
      {rest.length > 0 && (
        <div className="artifact-files">
          {rest.map((a) => (
            <a
              key={a.path}
              className="artifact-file"
              href={runArtifactUrl(runId, a.path)}
              target="_blank"
              rel="noreferrer"
            >
              {a.path}
            </a>
          ))}
        </div>
      )}
    </div>
  );
}

/** An experiment's detail view, rendered as right-pane tab content. Mount it
 *  keyed by `${experiment.id}:${view}` so per-view state resets on switch. */
export function DetailDrawer({
  experiment,
  project,
  view,
  runs,
  selectedRunId,
  onSelectRun,
  mergeParent = null,
}: {
  experiment: Experiment;
  /** Owning project — supplies owner/repo for the GitHub branch link. */
  project: Project;
  view: ExperimentView;
  runs: Run[];
  selectedRunId: string | null;
  onSelectRun: (id: string | null) => void;
  /** The merged-in experiment, when this node is a merge (dashed edge). */
  mergeParent?: Experiment | null;
}) {
  const expRuns = runs
    .filter((r) => r.experimentId === experiment.id)
    .sort((a, b) => b.createdAt - a.createdAt);

  // The parent experiment's latest sim aggregate — the baseline the delta
  // chips compare against (`runs` is project-wide, so it's all here).
  const parentAggregate =
    (experiment.parentExperimentId &&
      runs
        .filter(
          (r) =>
            r.experimentId === experiment.parentExperimentId &&
            r.kind === "sim" &&
            r.status === "done" &&
            r.metricsAggregate,
        )
        .sort((a, b) => b.createdAt - a.createdAt)[0]?.metricsAggregate) ||
    null;

  const body =
    view === "terminal" ? (
      <TerminalView
        experiment={experiment}
        expRuns={expRuns}
        parentAggregate={parentAggregate}
        selectedRunId={selectedRunId}
        onSelectRun={onSelectRun}
      />
    ) : view === "play" ? (
      <PlayView experiment={experiment} />
    ) : (
      <ChangesView experiment={experiment} project={project} />
    );
  return (
    <div className="exp-detail">
      <div className="exp-verdict-strip">
        <VerdictChips
          label="Experiment verdict"
          verdict={experiment.verdict as Verdict | null | undefined}
          notes={experiment.verdictNotes}
          onSave={(v, notes) => void setExperimentVerdict(experiment.id, v, notes).catch(() => {})}
        />
        {experiment.verdictNotes && (
          <span className="verdict-notes-preview" title={experiment.verdictNotes}>
            {experiment.verdictNotes}
          </span>
        )}
        {mergeParent && (
          <span
            className="verdict-notes-preview"
            title={`Branch ${mergeParent.branchName} was merged into this node at creation`}
          >
            merged from {mergeParent.title || mergeParent.slug}
          </span>
        )}
      </div>
      {/* .term-view/.drawer are inset-0 overlays — give them a positioned box
          below the verdict strip to fill. */}
      <div className="exp-detail-body">{body}</div>
    </div>
  );
}

/**
 * A run's terminal output filling the whole pane. The bar above carries the
 * stop button, the run's status and a history switcher — mirror of
 * openresearch.sh's ExperimentFullView TerminalView.
 */
function TerminalView({
  experiment,
  expRuns,
  parentAggregate,
  selectedRunId,
  onSelectRun,
}: {
  experiment: Experiment;
  expRuns: Run[];
  parentAggregate: Record<string, unknown> | null;
  selectedRunId: string | null;
  onSelectRun: (id: string | null) => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const historyRef = useRef<HTMLDivElement>(null);

  const selectedRun =
    (selectedRunId && expRuns.find((r) => r.id === selectedRunId)) || expRuns[0] || null;
  const live = selectedRun?.status === "running" || selectedRun?.status === "starting";
  // expRuns is newest-first, so the oldest run is #1. Number a run by its
  // position from the end of the list.
  const runNumber = (id: string) => {
    const idx = expRuns.findIndex((r) => r.id === id);
    return idx === -1 ? expRuns.length : expRuns.length - idx;
  };

  // When a new run starts while the tab is open, follow it live.
  const seenRunIds = useRef<Set<string> | null>(null);
  useEffect(() => {
    if (seenRunIds.current === null) {
      seenRunIds.current = new Set(expRuns.map((r) => r.id));
      return;
    }
    const fresh = expRuns.find((r) => !seenRunIds.current!.has(r.id));
    for (const r of expRuns) seenRunIds.current.add(r.id);
    if (fresh) onSelectRun(fresh.id);
  }, [expRuns, onSelectRun]);

  // Close the history dropdown on outside click.
  useEffect(() => {
    if (!historyOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!historyRef.current?.contains(e.target as Node)) setHistoryOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [historyOpen]);

  async function stop() {
    if (!selectedRun) return;
    setError(null);
    try {
      await cancelRun(selectedRun.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div className="term-view">
      <div className="term-bar">
        <div className="term-title" title={experiment.title || experiment.slug}>
          {experiment.title || experiment.slug}
        </div>
        <span style={{ flex: 1 }} />
        {error && <span className="error">{error}</span>}
        {selectedRun && !live && (
          <VerdictChips
            verdict={selectedRun.verdict ?? null}
            notes={selectedRun.verdictNotes}
            onSave={(v, notes) => void setRunVerdict(selectedRun.id, v, notes).catch(() => {})}
          />
        )}
        {live && (
          <button className="btn sm ghost" onClick={() => void stop()}>
            <CircleStop size={13} />
            Stop
          </button>
        )}
        {expRuns.length > 0 && selectedRun && (
          <div className="run-history" ref={historyRef}>
            <button
              className="run-picker"
              title="Switch run"
              onClick={() => setHistoryOpen((v) => !v)}
            >
              <span className="run-label">Run {runNumber(selectedRun.id)}</span>
              <StatusBadge status={selectedRun.status} />
              <ChevronDown size={14} className="run-picker-chev" />
            </button>
            {historyOpen && (
              <div className="history-menu">
                {expRuns.map((r) => (
                  <button
                    key={r.id}
                    className={`history-item ${r.id === selectedRun?.id ? "active" : ""}`}
                    onClick={() => {
                      onSelectRun(r.id);
                      setHistoryOpen(false);
                    }}
                  >
                    <span className="run-label">Run {runNumber(r.id)}</span>
                    <StatusBadge status={r.status} />
                    <span className="when">{timeAgo(r.createdAt)}</span>
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {selectedRun?.metricsAggregate && (
        <div className="results-strip">
          <MetricChips
            aggregate={selectedRun.metricsAggregate}
            parentAggregate={parentAggregate}
          />
        </div>
      )}
      {selectedRun && (
        <ArtifactGallery
          key={`art-${selectedRun.id}`}
          runId={selectedRun.id}
          terminal={!live}
        />
      )}

      <div className="term-fill">
        {selectedRun ? (
          // Key by run id so switching runs in the history dropdown remounts
          // the terminal with the selected run's output.
          <LogTerminal key={selectedRun.id} runId={selectedRun.id} />
        ) : (
          <div className="term-empty">No runs yet — ask the agent to launch one.</div>
        )}
      </div>
    </div>
  );
}

/** The branch's changes: a commit picker + diff, including uncommitted edits. */
function ChangesView({ experiment, project }: { experiment: Experiment; project: Project }) {
  const [commits, setCommits] = useState<CommitInfo[] | null>(null);
  const [changesError, setChangesError] = useState<string | null>(null);
  const [workingTree, setWorkingTree] = useState<WorkingTree | null>(null);
  const [selection, setSelection] = useState<string | null>(null);
  const [commitDiffs, setCommitDiffs] = useState<Record<string, DiffState>>({});

  const uncommittedAvailable = Boolean(
    workingTree &&
      workingTree.diff.trim() !== "" &&
      workingTree.experimentId === experiment.id,
  );

  async function loadChanges() {
    setChangesError(null);
    try {
      const [commitList, wt] = await Promise.all([
        listExperimentCommits(experiment.id),
        getWorkingTree(experiment.projectId),
      ]);
      setCommits(commitList);
      setWorkingTree(wt);
      setSelection((prev) => {
        if (prev !== null) return prev;
        const wtActive = wt.diff.trim() !== "" && wt.experimentId === experiment.id;
        if (wtActive) return UNCOMMITTED;
        return commitList[0]?.sha ?? null;
      });
    } catch (err) {
      setChangesError(err instanceof Error ? err.message : String(err));
    }
  }

  // First open loads commits + working tree.
  useEffect(() => {
    if (commits === null && !changesError) void loadChanges();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [commits, changesError, experiment.id]);

  // Poll the working tree while the view is visible.
  useEffect(() => {
    const timer = setInterval(() => {
      getWorkingTree(experiment.projectId)
        .then(setWorkingTree)
        .catch(() => {
          // transient; next tick retries
        });
    }, 5000);
    return () => clearInterval(timer);
  }, [experiment.projectId]);

  // If the uncommitted diff disappears from under the selection, fall back.
  useEffect(() => {
    if (selection === UNCOMMITTED && workingTree && !uncommittedAvailable) {
      setSelection(commits?.[0]?.sha ?? null);
    }
  }, [selection, workingTree, uncommittedAvailable, commits]);

  // Lazily fetch the selected commit's diff, cached per sha.
  useEffect(() => {
    if (!selection || selection === UNCOMMITTED) return;
    if (commitDiffs[selection]) return;
    const sha = selection;
    setCommitDiffs((m) => ({ ...m, [sha]: { loading: true } }));
    getCommitDiff(experiment.id, sha)
      .then((payload) => setCommitDiffs((m) => ({ ...m, [sha]: { loading: false, payload } })))
      .catch((err) =>
        setCommitDiffs((m) => ({
          ...m,
          [sha]: { loading: false, error: err instanceof Error ? err.message : String(err) },
        })),
      );
  }, [selection, commitDiffs, experiment.id]);

  const noChanges = !uncommittedAvailable && (commits?.length ?? 0) === 0;

  return (
    <div className="drawer">
      <div className="drawer-body">
        <div className="drawer-section">
          <div className="changes-branch">
            <span className="ctl-label">Branch</span>
            <BranchPill
              owner={project.githubOwner}
              repo={project.githubRepo}
              branch={experiment.branchName}
            />
          </div>
          {changesError ? (
            <div className="error">{changesError}</div>
          ) : commits === null ? (
            <div className="changes-note">Loading changes…</div>
          ) : noChanges ? (
            <div className="changes-note">
              No changes yet — the agent hasn't committed on this branch.
            </div>
          ) : (
            <>
              <div className="commit-picker">
                <span className="ctl-label">Commit</span>
                {selection === UNCOMMITTED && <span className="uncommitted-dot" />}
                <select
                  className="input sm"
                  value={selection ?? ""}
                  onChange={(e) => setSelection(e.target.value)}
                >
                  {uncommittedAvailable && (
                    <option value={UNCOMMITTED}>● Uncommitted changes</option>
                  )}
                  {commits.map((c) => (
                    <option key={c.sha} value={c.sha}>
                      {c.sha.slice(0, 7)} — {c.subject}
                    </option>
                  ))}
                </select>
                <button
                  className="icon-btn"
                  title="Refresh"
                  aria-label="Refresh"
                  onClick={() => void loadChanges()}
                >
                  <RotateCw size={14} />
                </button>
              </div>
              <div style={{ marginTop: 10 }}>
                {selection === UNCOMMITTED && workingTree ? (
                  workingTree.truncated ? (
                    <div className="truncated-notice">
                      <h4>Diff too large to display</h4>
                      <p>The uncommitted diff is too large to display. View it locally with git.</p>
                    </div>
                  ) : (
                    <GitDiff diff={workingTree.diff} />
                  )
                ) : selection ? (
                  <DiffView state={commitDiffs[selection]} />
                ) : (
                  <div className="changes-note">Select a commit to view its diff.</div>
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
