import {
  Background,
  BackgroundVariant,
  Handle,
  Position,
  ReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import { FolderTree, GitBranch, Play, Terminal } from "lucide-react";
import { GitHubMark } from "./BackendLogos";
import { memo, useMemo, useRef } from "react";
import { githubBranchUrl, timeAgo, type Experiment, type Project, type Run } from "../api";
import type { ExperimentView } from "./DetailDrawer";
import { ExpHoverCard, dismissTreeHoverCards, useHoverIntent } from "./ExpHoverCard";
import { StatusBadge } from "./StatusBadge";

const NODE_W = 264;
const NODE_H = 132;
const GAP_X = 44;
const GAP_Y = 72;
const MAX_SQUARES = 8;

type ExpNodeData = {
  exp: Experiment;
  latestRun: Run | null;
  runs: Run[]; // oldest → newest
  isBaseline: boolean;
  parentSlug: string | null;
  githubOwner: string;
  githubRepo: string;
  onOpenView: (id: string, view: ExperimentView) => void;
  onOpenCodeBranch: (branch: string) => void;
};
type ExpFlowNode = Node<ExpNodeData, "exp">;

interface TreeNode {
  exp: Experiment;
  children: TreeNode[];
}

function buildForest(experiments: Experiment[]): TreeNode[] {
  const byId = new Map(experiments.map((e) => [e.id, { exp: e, children: [] as TreeNode[] }]));
  const roots: TreeNode[] = [];
  for (const e of experiments) {
    const node = byId.get(e.id)!;
    const parent = e.parentExperimentId ? byId.get(e.parentExperimentId) : undefined;
    if (parent) parent.children.push(node);
    else roots.push(node);
  }
  const byCreated = (a: TreeNode, b: TreeNode) => a.exp.createdAt - b.exp.createdAt;
  const sortRec = (n: TreeNode) => {
    n.children.sort(byCreated);
    n.children.forEach(sortRec);
  };
  roots.sort(byCreated);
  roots.forEach(sortRec);
  return roots;
}

function subtreeWidth(node: TreeNode): number {
  if (node.children.length === 0) return NODE_W;
  const cw =
    node.children.reduce((s, c) => s + subtreeWidth(c), 0) + GAP_X * (node.children.length - 1);
  return Math.max(NODE_W, cw);
}

function runSquareClass(status: string): string {
  if (status === "done") return "pass";
  if (status === "failed") return "fail";
  if (status === "running" || status === "starting") return "live";
  return "other";
}

const ExpNode = memo(function ExpNode({ data }: NodeProps<ExpFlowNode>) {
  const { exp, latestRun, runs, isBaseline, parentSlug, githubOwner, githubRepo, onOpenView, onOpenCodeBranch } = data;
  const status = latestRun?.status;
  const live = status === "running" || status === "starting";
  const kind = isBaseline
    ? "Idea"
    : live
      ? "Building"
      : exp.mergeParentExperimentId
        ? "Merge"
        : "Prototype";
  const squares = runs.slice(-MAX_SQUARES);

  // `data` is rebuilt on every experiments/runs change (a superset of the
  // re-layouts that matter), so it doubles as the hover card's re-measure
  // key. Canvas pan/zoom dismissal arrives via dismissTreeHoverCards, wired
  // to the ReactFlow onMoveStart prop below.
  const rootRef = useRef<HTMLDivElement>(null);
  const hover = useHoverIntent(rootRef, data);

  return (
    <div
      ref={rootRef}
      className={`exp-node ${live ? "live" : ""}`}
      onMouseEnter={hover.onMouseEnter}
      onMouseLeave={hover.onMouseLeave}
    >
      <Handle type="target" position={Position.Top} />
      <div className="node-eyebrow">
        <span className="node-eyebrow-left">
          {kind}
          <a
            className="icon-btn node-gh"
            title={`Open ${exp.branchName} on GitHub`}
            aria-label={`Open ${exp.branchName} on GitHub`}
            href={githubBranchUrl(githubOwner, githubRepo, exp.branchName)}
            target="_blank"
            rel="noopener noreferrer"
            onClick={(e) => e.stopPropagation()}
          >
            <GitHubMark size={12} />
          </a>
        </span>
        <span style={{ display: "inline-flex", gap: 6 }}>
          {exp.verdict && <StatusBadge status={exp.verdict} />}
          <StatusBadge status={status ?? "idle"} />
        </span>
      </div>
      <div className="node-head">
        <span className="node-slug">{exp.slug}</span>
      </div>
      {(exp.title || exp.description) && (
        <div className="node-title">{exp.title || exp.description}</div>
      )}
      {(() => {
        // Surface the latest run's ingested metrics (e.g. the eval sim's
        // alignment/confidence) right on the card, not just in the run drawer.
        const agg =
          (latestRun?.metricsAggregate as Record<string, unknown> | undefined) ??
          [...runs].reverse().find((r) => r.metricsAggregate)?.metricsAggregate;
        const entries = agg
          ? Object.entries(agg).filter(([, v]) => typeof v === "number")
          : [];
        if (!entries.length) return null;
        return (
          <div className="node-metrics">
            {entries.slice(0, 5).map(([k, v]) => (
              <span key={k} className="node-metric" title={k}>
                <span className="node-metric-k">
                  {k.replace(/_align$/, "").replace(/_/g, " ")}
                </span>
                <span className="node-metric-v">
                  {(v as number).toFixed(2).replace(/\.?0+$/, "")}
                </span>
              </span>
            ))}
          </div>
        );
      })()}
      <div className="node-meta">
        <span>Runs</span>
        {squares.length > 0 ? (
          <span className="run-squares">
            {squares.map((run) => (
              <span key={run.id} className={`run-sq ${runSquareClass(run.status)}`} title={run.status} />
            ))}
          </span>
        ) : (
          <span>no runs</span>
        )}
        <span style={{ flex: 1 }} />
        {latestRun && <span>{timeAgo(latestRun.createdAt)}</span>}
      </div>
      {/* Direct view shortcuts — changes and code always, logs once there's a run. */}
      <div className="node-actions" onClick={(e) => e.stopPropagation()}>
        <button
          className="node-action"
          title="Build & play this experiment"
          onClick={() => onOpenView(exp.id, "play")}
        >
          <Play size={13} />
          Play
        </button>
        <button
          className="node-action"
          title="Open changes"
          onClick={() => onOpenView(exp.id, "changes")}
        >
          <GitBranch size={13} />
          Changes
        </button>
        {runs.length > 0 && (
          <button
            className="node-action"
            title="Open logs"
            onClick={() => onOpenView(exp.id, "terminal")}
          >
            <Terminal size={13} />
            Logs
          </button>
        )}
        <button
          className="node-action"
          title={`Browse code on ${exp.branchName}`}
          onClick={() => onOpenCodeBranch(exp.branchName)}
        >
          <FolderTree size={13} />
          Code
        </button>
      </div>
      <Handle type="source" position={Position.Bottom} />
      {/* Node and card share one leave handler — React's enter/leave pairing
        * across the portal (fiber-tree walk) relies on it; don't split them. */}
      {hover.rect && (
        <ExpHoverCard
          exp={exp}
          runs={runs}
          latestRun={latestRun}
          parentSlug={parentSlug}
          anchor={hover.rect}
          onMouseEnter={hover.keepOpen}
          onMouseLeave={hover.onMouseLeave}
        />
      )}
    </div>
  );
});

const nodeTypes = { exp: ExpNode };

const defaultEdgeOptions = {
  type: "default", // bezier
  style: { stroke: "var(--text)", strokeWidth: 1.5, opacity: 0.3 },
};

export function TreeView({
  experiments,
  runs,
  project,
  onOpenView,
  onOpenCodeBranch,
}: {
  experiments: Experiment[];
  runs: Run[];
  /** Owning project — supplies owner/repo for the GitHub branch links. */
  project: Project;
  /** Open an experiment view as a right-pane tab (card shortcut buttons). */
  onOpenView: (id: string, view: ExperimentView) => void;
  /** Browse an experiment branch's code in the project-level Code tab. */
  onOpenCodeBranch: (branch: string) => void;
}) {
  const { nodes, edges } = useMemo(() => {
    const runsByExp = new Map<string, Run[]>();
    for (const run of runs) {
      const list = runsByExp.get(run.experimentId);
      if (list) list.push(run);
      else runsByExp.set(run.experimentId, [run]);
    }
    for (const list of runsByExp.values()) list.sort((a, b) => a.createdAt - b.createdAt);

    const nodes: ExpFlowNode[] = [];
    const edges: Edge[] = [];
    const roots = buildForest(experiments);
    const slugById = new Map(experiments.map((e) => [e.id, e.slug]));

    function layout(node: TreeNode, cx: number, y: number) {
      const expRuns = runsByExp.get(node.exp.id) ?? [];
      nodes.push({
        id: node.exp.id,
        type: "exp",
        position: { x: cx - NODE_W / 2, y },
        data: {
          exp: node.exp,
          latestRun: expRuns[expRuns.length - 1] ?? null,
          runs: expRuns,
          isBaseline: !node.exp.parentExperimentId,
          parentSlug: node.exp.parentExperimentId
            ? (slugById.get(node.exp.parentExperimentId) ?? null)
            : null,
          githubOwner: project.githubOwner,
          githubRepo: project.githubRepo,
          onOpenView,
          onOpenCodeBranch,
        },
      });
      if (node.children.length === 0) return;
      const totalW =
        node.children.reduce((s, c) => s + subtreeWidth(c), 0) +
        GAP_X * (node.children.length - 1);
      let x = cx - totalW / 2;
      for (const child of node.children) {
        const cw = subtreeWidth(child);
        edges.push({
          id: `e-${node.exp.id}-${child.exp.id}`,
          source: node.exp.id,
          target: child.exp.id,
        });
        layout(child, x + cw / 2, y + NODE_H + GAP_Y);
        x += cw + GAP_X;
      }
    }

    let rx = 0;
    for (const root of roots) {
      const w = subtreeWidth(root);
      layout(root, rx + w / 2, 0);
      rx += w + GAP_X;
    }
    // Merge lineage: a dashed second edge from the merged-in experiment to
    // the merge node, on top of the tree's parent edges.
    const placed = new Set(nodes.map((n) => n.id));
    for (const e of experiments) {
      if (e.mergeParentExperimentId && placed.has(e.mergeParentExperimentId) && placed.has(e.id)) {
        edges.push({
          id: `m-${e.mergeParentExperimentId}-${e.id}`,
          source: e.mergeParentExperimentId,
          target: e.id,
          style: { stroke: "var(--text)", strokeWidth: 1.5, opacity: 0.45, strokeDasharray: "5 4" },
        });
      }
    }
    return { nodes, edges };
  }, [experiments, runs, onOpenView, onOpenCodeBranch, project.githubOwner, project.githubRepo]);

  if (experiments.length === 0) {
    return (
      <div className="empty-state empty-state-cta">
        <p className="empty-state-title">No ideas yet</p>
        <p className="empty-state-hint">Tell the producer in chat what kind of game you want to make — it'll capture your first idea.</p>
      </div>
    );
  }

  return (
    <ReactFlow
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      defaultEdgeOptions={defaultEdgeOptions}
      nodesDraggable={false}
      nodesConnectable={false}
      nodesFocusable={false}
      onMoveStart={dismissTreeHoverCards}
      minZoom={0.15}
      fitView
      fitViewOptions={{ padding: 0.25, maxZoom: 1 }}
    >
      <Background variant={BackgroundVariant.Dots} color="var(--dots-strong)" gap={28} size={1.6} />
    </ReactFlow>
  );
}
