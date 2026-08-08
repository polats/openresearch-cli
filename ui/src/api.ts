// Typed client for the orx up local HTTP API (/api/*). All wire JSON is camelCase.

export interface Project {
  id: string;
  name: string;
  slug: string;
  githubOwner: string;
  githubRepo: string;
  baselineBranch: string;
  repoPath: string;
  /** Absolute path of the project's files dir (`<data dir>/files/<slug>`),
   *  non-canonical to match the paths agents inline into chat. Lets the UI
   *  recognize a files-dir link and route it to the files endpoints. */
  filesDir: string;
  runCommand?: string | null;
  /** arXiv id the project starts from (versionless). */
  paperId?: string | null;
  /** Agent persona wire id ("research" | "game-designer" | "idea-foundry"); null = research. */
  persona?: string | null;
  /** Automatic [orx] prompt switches; null/absent = all off. */
  autoPrompts?: AutoPrompts | null;
  createdAt: number;
  updatedAt: number;
}

/** Per-project switches for the run-watcher's automatic chat prompts.
 * All default off — unsolicited agent turns proved intrusive. */
export interface AutoPrompts {
  /** Play session ended → ask the user for a verdict. */
  playSession?: boolean;
  /** Play build failed → tell the agent to fix the branch. */
  playBuildFailed?: boolean;
  /** Job/sim run completed → reconcile and continue the loop. */
  runCompleted?: boolean;
}

export interface Experiment {
  id: string;
  projectId: string;
  parentExperimentId?: string | null;
  slug: string;
  branchName: string;
  title?: string | null;
  description?: string | null;
  runCommand: string;
  agentStatus: string;
  createdAt: number;
  updatedAt: number;
  /** Standing human verdict — set explicitly, never derived from runs. */
  verdict?: Verdict | null;
  verdictNotes?: string | null;
  verdictAt?: number | null;
  /** Second parent: experiment whose branch was merged in at creation. */
  mergeParentExperimentId?: string | null;
  /** Entry page under /play/<id>/ (path + optional query); null = index.html. */
  playEntry?: string | null;
  /** Chat session that created this experiment; null for dashboard/legacy rows. */
  chatSessionId?: string | null;
}

export type RunStatus = "starting" | "running" | "done" | "failed" | "cancelled";

/** What kind of evaluation a run is (see the Phase 1 spec). */
export type RunKind = "job" | "play" | "sim" | "verify" | "ladder";

/** Human judgment on a run or experiment. */
export type Verdict = "keep" | "kill" | "iterate";

export interface Run {
  id: string;
  experimentId: string;
  projectId: string;
  status: RunStatus;
  kind: RunKind;
  /** The `aggregate` slice of the run's ingested metrics (numbers, small);
   *  the full document is `getRunMetrics`. */
  metricsAggregate?: Record<string, unknown>;
  verdict?: Verdict;
  verdictNotes?: string;
  verdictAt?: number;
  backend?: Record<string, unknown> | null;
  command?: string | null;
  commitSha?: string | null;
  resultMarkdown?: string | null;
  createdAt: number;
  updatedAt: number;
  endedAt?: number | null;
  exitCode?: number | null;
  /** Watcher (supervisor process) state, present only on live runs:
   *  "alive" = its heartbeat is fresh; "lost" = it stopped beating
   *  (reboot, kill) and the status can no longer update on its own. */
  watcher?: "alive" | "lost";
}

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    let message = text;
    try {
      const parsed = JSON.parse(text) as { error?: string };
      if (parsed.error) message = parsed.error;
    } catch {
      // non-JSON body — show it raw
    }
    throw new Error(message || `HTTP ${res.status}`);
  }
  return (await res.json()) as T;
}

const get = <T>(url: string) => fetch(url).then((r) => json<T>(r));
const post = <T>(url: string, body?: unknown) =>
  fetch(url, {
    method: "POST",
    headers: body === undefined ? {} : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  }).then((r) => json<T>(r));
const patch = <T>(url: string, body: unknown) =>
  fetch(url, {
    method: "PATCH",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  }).then((r) => json<T>(r));

export const listProjects = () =>
  get<{ projects: Project[] }>("/api/projects").then((r) => r.projects);

export interface NewProject {
  name: string;
  githubOwner?: string;
  githubRepo?: string;
  githubOrganization?: string;
  baselineBranch?: string;
  runCommand?: string;
  /** arXiv id the project starts from (versionless). */
  paperId?: string;
  /** Create a blank private repo on the user's GitHub account instead. */
  createRepo?: boolean;
  /** Fork-by-copy the repo into a fresh `<repo>-<hash>` repo on the user's
   * account. Applied automatically when they lack push access. */
  forkRepo?: boolean;
  /** Publish this local checkout — which has no `origin` — to a new GitHub repo
   * and push its history. Only send it when the user has explicitly opted in;
   * it is the one create path that writes outside orx. */
  publishLocalPath?: string;
}

/** What is at a local path, as the New Project form needs to explain it. Five
 *  shapes, because each has a different fix. */
export type LocalRepoInfo =
  | { kind: "missing" }
  | { kind: "notARepo" }
  | { kind: "noRemote"; currentBranch?: string | null; suggestedName?: string | null }
  | { kind: "foreignRemote"; remoteUrl: string }
  | {
      kind: "github";
      owner: string;
      repo: string;
      remoteUrl: string;
      currentBranch?: string | null;
    };

/** Inspect a directory on this machine. Read-only: reports the shape of what's
 *  there, never file contents. */
export const inspectLocalRepo = (path: string) =>
  get<LocalRepoInfo>(`/api/local-repo?path=${encodeURIComponent(path)}`);

export const createProject = (body: NewProject) =>
  post<{ project: Project }>("/api/projects", body).then((r) => r.project);

export interface PaperHit {
  paperId: string;
  title: string;
  snippet?: string | null;
}

export interface ResolvedPaper {
  paperId: string;
  title?: string | null;
  repoUrl?: string | null;
  repoStars?: number | null;
}

export const searchPapers = (q: string) =>
  get<{ papers: PaperHit[] }>(`/api/papers/search?q=${encodeURIComponent(q)}`).then(
    (r) => r.papers,
  );

/** The signed-in GitHub login, for naming the account a new repo lands on.
 * `login` is null when there's no usable token. */
export const githubAccount = () => get<{ login: string | null }>("/api/github/account");

/** Whether the stored credentials can push to a repo. An unanswerable check
 * (no token / API hiccup) reports `true`, matching the server's own fallback,
 * so an outage never shows a fork choice the server wouldn't honour. */
export const repoAccess = (owner: string, repo: string) =>
  get<{ canPush: boolean }>(
    `/api/github/repo-access?owner=${encodeURIComponent(owner)}&repo=${encodeURIComponent(repo)}`,
  );

/** Resolve an arXiv id / URL to title + linked GitHub repo. May take a few
 * seconds for papers alphaXiv hasn't indexed yet (it scrapes arXiv on a miss). */
export const resolvePaper = (id: string) =>
  get<{ paper: ResolvedPaper }>(`/api/papers/resolve?id=${encodeURIComponent(id)}`).then(
    (r) => r.paper,
  );

export const updateProject = (
  projectId: string,
  body: {
    runCommand?: string;
    name?: string;
    persona?: string;
    autoPrompts?: AutoPrompts;
    /** Branch new baselines fork from (must exist on origin). */
    baselineBranch?: string;
  },
) => patch<{ project: Project }>(`/api/projects/${projectId}`, body).then((r) => r.project);

/** The repo's pickable fork-point branches (origin, minus orx/* experiment
 * branches) plus the project's current baseline. */
export const listProjectBranches = (projectId: string) =>
  get<{ branches: string[]; baseline: string }>(`/api/projects/${projectId}/branches`);

export interface GithubRepo {
  fullName: string;
  private: boolean;
}

/** The signed-in user's repos, most recently pushed first (empty without a
 * GitHub token) — backs the New Project repo autocomplete. */
export const listGithubRepos = () =>
  get<{ repos: GithubRepo[] }>("/api/github/repos").then((r) => r.repos);

export interface PersonaSkill {
  name: string;
  description: string;
  content: string;
}

export interface PersonaInfo {
  id: string;
  label: string;
  description: string;
  /** The persona's system-prompt template ({token} placeholders visible). */
  systemPrompt: string;
  skills: PersonaSkill[];
}

export const getPersonas = () =>
  get<{ personas: PersonaInfo[] }>("/api/personas").then((r) => r.personas);

/** A subagent dispatch the orchestrator suggested, awaiting the human's OK. */
export interface AgentProposal {
  id: string;
  projectId: string;
  parentExperimentId?: string | null;
  persona?: string | null;
  harness?: string | null;
  model?: string | null;
  task: string;
  why?: string | null;
  /** "pending" | "approved" | "dismissed". */
  status: string;
  sessionId?: string | null;
  /** The orchestrator session this suggestion belongs to (renders in its chat). */
  parentSessionId?: string | null;
  createdAt: number;
  updatedAt: number;
}

export const listProposals = (projectId: string) =>
  get<{ proposals: AgentProposal[] }>(`/api/projects/${projectId}/proposals`).then(
    (r) => r.proposals,
  );

/** Approve a proposal; the passed picks override the orchestrator's suggestion. */
export const approveProposal = (
  id: string,
  choice: { persona?: string; harness?: string; model?: string },
) =>
  post<{ session: ChatSession }>(`/api/agent/proposals/${id}/approve`, choice).then(
    (r) => r.session,
  );

export const dismissProposal = (id: string) =>
  post<{ ok: boolean }>(`/api/agent/proposals/${id}/dismiss`, {});

/** Record a visit: bumps the project's updatedAt, which drives the recency sort. */
export const openProject = (projectId: string) =>
  post<{ project: Project }>(`/api/projects/${projectId}/open`).then((r) => r.project);

export const deleteProject = (projectId: string) =>
  fetch(`/api/projects/${projectId}`, { method: "DELETE" }).then(async (r) => {
    if (!r.ok) {
      const body = await r.json().catch(() => null);
      throw new Error(body?.error ?? `delete failed (${r.status})`);
    }
  });

export const listExperiments = (projectId: string) =>
  get<{ experiments: Experiment[] }>(`/api/projects/${projectId}/experiments`).then(
    (r) => r.experiments,
  );

export const listRuns = (projectId: string) =>
  get<{ runs: Run[] }>(`/api/projects/${projectId}/runs`).then((r) => r.runs);

/** A run viewed as compute: every run across all projects, tagged with the
 *  name of the project that launched it. `projectName` is enriched only on the
 *  /api/instances snapshot — it is absent from the `run.updated` SSE payload. */
export interface Instance extends Run {
  projectName?: string;
}

export const listInstances = () =>
  get<{ instances: Instance[] }>("/api/instances").then((r) => r.instances);

/** Respawn `orx supervise` for every live run whose watcher stopped beating;
 *  returns the ids re-attached. No-op (empty list) when all watchers are alive. */
export const reconcileInstances = () =>
  post<{ reattached: string[] }>("/api/instances/reconcile").then((r) => r.reattached);

export interface NewExperiment {
  /** Omit on an empty project to create the baseline root; once a root
   *  exists, an omitted parent attaches the node under the oldest root. */
  parentExperimentId?: string;
  /** Force a new baseline root even when the project already has one. */
  baseline?: boolean;
  slug?: string;
  title?: string;
  description?: string;
  runCommand?: string;
  /** Merge this experiment's branch into the new node (second parent). */
  mergeParentExperimentId?: string;
}

export const createExperiment = (projectId: string, body: NewExperiment) =>
  post<{ experiment: Experiment }>(`/api/projects/${projectId}/experiments`, body).then(
    (r) => r.experiment,
  );

export const startRun = (
  experimentId: string,
  body: {
    /** Omit to launch on the default compute target (Settings → Compute);
     * with no default set the server falls back to `hf`. */
    backend?: "local" | "hf" | "modal" | "k8s" | "ssh" | "slurm" | "ray" | "openresearch";
    flavor?: string;
    manifest?: string;
    timeout?: string;
    /** ssh host alias, or the Slurm login node; defaults to the slurm settings' host. */
    host?: string;
    /** Org to bill the box to (openresearch only; omit = the sole org). */
    org?: string;
  } = {},
) => post<{ run: Run }>(`/api/experiments/${experimentId}/run`, body).then((r) => r.run);

export const cancelRun = (runId: string) => post<{ ok: boolean }>(`/api/runs/${runId}/cancel`);

// --- verdicts, metrics, artifacts, play builds (game-design experiments) ----

export const setRunVerdict = (runId: string, verdict: Verdict | null, notes?: string) =>
  post<{ run: Run }>(`/api/runs/${runId}/verdict`, { verdict, notes }).then((r) => r.run);

export const setExperimentVerdict = (expId: string, verdict: Verdict | null, notes?: string) =>
  post<{ experiment: Experiment }>(`/api/experiments/${expId}/verdict`, { verdict, notes }).then(
    (r) => r.experiment,
  );

export const getRunMetrics = (runId: string) =>
  get<{ metrics: Record<string, unknown> | null }>(`/api/runs/${runId}/metrics`).then(
    (r) => r.metrics,
  );

export interface RunArtifact {
  path: string;
  size: number;
  contentType: string;
}

export const listRunArtifacts = (runId: string) =>
  get<{ artifacts: RunArtifact[]; truncated: boolean }>(`/api/runs/${runId}/artifacts`).then(
    (r) => r.artifacts,
  );

export const runArtifactUrl = (runId: string, path: string) =>
  `/api/runs/${runId}/artifacts/file?path=${encodeURIComponent(path)}`;

export interface PlayBuild {
  /** "ready" = a fresh build is being served; "building" = a build run is in flight. */
  state: "ready" | "building";
  url: string;
  runId?: string;
}

/** Build (or reuse) the experiment's playable; idempotent. */
export const startPlayBuild = (expId: string) =>
  post<PlayBuild>(`/api/experiments/${expId}/play-build`);

/** The playable URL for an experiment (its entry page when one is set). */
export const playUrl = (exp: Pick<Experiment, "id" | "playEntry">) =>
  `/play/${exp.id}/${exp.playEntry ?? ""}`;

export const setExperimentPlayEntry = (expId: string, playEntry: string | null) =>
  patch<{ experiment: Experiment }>(`/api/experiments/${expId}`, { playEntry }).then(
    (r) => r.experiment,
  );

/** Start-or-keep-alive the experiment's play session (the play tab calls it
 *  every 10s while mounted). Errors while the build isn't ready yet. */
export const beatPlaySession = (expId: string) =>
  post<{ run: Run }>(`/api/experiments/${expId}/play-session/beat`).then((r) => r.run);

/** End the open play session; the verdict (if any) lands on the session run.
 *  No open session is a no-op. */
export const endPlaySession = (expId: string, verdict?: Verdict | null, notes?: string) =>
  post<{ run: Run | null }>(`/api/experiments/${expId}/play-session/end`, {
    verdict: verdict ?? null,
    notes,
  }).then((r) => r.run);

export interface LogChunk {
  dataBase64: string;
  nextOffset: number;
  eof: boolean;
}

export const fetchLog = (runId: string, offset: number) =>
  get<LogChunk>(`/api/runs/${runId}/log?offset=${offset}`);

export interface DiffPayload {
  diff: string;
  truncated: boolean;
  bytesRead: number;
  byteLimit: number;
}

export interface CommitInfo {
  sha: string;
  subject: string;
  committedAt: number; // unix seconds
}

export interface WorkingTree {
  branch: string | null;
  experimentId: string | null;
  diff: string;
  truncated: boolean;
}

export const getRunDiff = (runId: string) => get<DiffPayload>(`/api/runs/${runId}/diff`);

export const listExperimentCommits = (experimentId: string) =>
  get<{ commits: CommitInfo[] }>(`/api/experiments/${experimentId}/commits`).then(
    (r) => r.commits,
  );

export const getCommitDiff = (experimentId: string, sha: string) =>
  get<DiffPayload>(`/api/experiments/${experimentId}/commits/${sha}/diff`);

export const getWorkingTree = (projectId: string) =>
  get<WorkingTree>(`/api/projects/${projectId}/working-tree`);

/** Which source answered a checkout read: a session's live worktree, the hub
 * clone (also the worktree-pruned fallback), or a branch's committed tree. */
export type CheckoutRoot = "worktree" | "clone" | "branch";

/** Source selector for checkout reads: `ref` picks a branch's committed
 * state; `sessionId` picks the session's live worktree; neither picks the hub
 * clone. Don't send both — the file endpoint ignores `sessionId` under `ref`,
 * but code-tree rejects the combination outright. */
export interface CheckoutRef {
  sessionId?: string;
  ref?: string;
}

const checkoutQuery = (opts: CheckoutRef, params: URLSearchParams = new URLSearchParams()) => {
  if (opts.sessionId) params.set("sessionId", opts.sessionId);
  if (opts.ref) params.set("ref", opts.ref);
  return params;
};

export interface ProjectFile {
  path: string;
  content: string;
  truncated: boolean;
  notFound: boolean;
  root: CheckoutRoot;
}

/** One file from the project — a branch's committed copy when `ref` is given,
 * else a chat session's worktree, else the hub clone — capped server-side
 * (~512 KB). */
export const getProjectFile = (projectId: string, path: string, opts: CheckoutRef = {}) =>
  get<ProjectFile>(
    `/api/projects/${projectId}/file?${checkoutQuery(opts, new URLSearchParams({ path }))}`,
  );

export interface CodeTree {
  root: CheckoutRoot;
  /** The listed branch (`ref` mode), else the checked-out branch, else null
   * (detached HEAD). */
  branch: string | null;
  /** Repo-relative file paths (gitignored trees excluded), sorted. */
  entries: string[];
  /** True when the listing hit the server-side cap (20,000 entries). */
  truncated: boolean;
}

/** Flat file listing of the project — a branch's committed tree when `ref` is
 * given, else a chat session's live worktree when `sessionId` is given, else
 * the hub clone's checkout — plus the branch name. `ref` and `sessionId` are
 * mutually exclusive (the server rejects both). */
export const getCodeTree = (projectId: string, opts: CheckoutRef = {}) => {
  const qs = checkoutQuery(opts).toString();
  return get<CodeTree>(`/api/projects/${projectId}/code-tree${qs ? `?${qs}` : ""}`);
};

/** How a file in a session's worktree differs from the diff base. Lowercase to
 * match the server's serialization and the single-letter badges the UI draws. */
export type ChangedStatus = "added" | "modified" | "deleted" | "renamed" | "untracked";

export interface ChangedFile {
  path: string;
  status: ChangedStatus;
  /** Pre-rename path — present only for `renamed` entries. */
  oldPath?: string;
}

/** Live view of a chat session's private worktree. `exists: false` when the
 * agent hasn't started yet (the worktree is created lazily on the first turn)
 * or was pruned — the remaining fields are then absent. `files` is the
 * complete change list even when `diff` truncates (they come from separate git
 * passes); `diff` is the working tree against the baseline merge-base, with
 * untracked files rendered as new-file diffs. */
export interface SessionWorktree {
  exists: boolean;
  /** Checked-out branch, or null when detached at the baseline tip. */
  branch?: string | null;
  baselineBranch?: string;
  baseSha?: string;
  files?: ChangedFile[];
  diff?: DiffPayload;
}

export const getSessionWorktree = (sessionId: string) =>
  get<SessionWorktree>(`/api/chat/sessions/${sessionId}/worktree`);

/** A GitHub `tree` URL for a branch. Branch names contain `/` (`orx/<slug>`),
 * so encode each path segment — never the whole string, which would escape the
 * slashes. Unpushed branches 404 on GitHub, which is acceptable. */
export const githubBranchUrl = (owner: string, repo: string, branch: string) =>
  `https://github.com/${owner}/${repo}/tree/${branch
    .split("/")
    .map(encodeURIComponent)
    .join("/")}`;

export type HfTokenSource = "env" | "openresearchEnv" | "hfCache";

export interface HfSettings {
  configured: boolean;
  source: HfTokenSource | null;
  maskedToken: string | null;
  valid: boolean;
  username: string | null;
  jobsWrite: boolean | null;
}

export const getHfSettings = () => get<HfSettings>("/api/settings/hf");

export const saveHfToken = (token: string) => post<HfSettings>("/api/settings/hf", { token });

// --- settings: kubernetes -----------------------------------------------------

export interface K8sPreflight {
  kubectlFound: boolean;
  reachable: boolean;
  canCreateJobs: boolean;
  error?: string;
}

export interface K8sSettings {
  configured: boolean;
  contexts: string[];
  currentContext: string | null;
  context: string | null;
  namespace: string;
  preflight: K8sPreflight;
}

export const getK8sSettings = () => get<K8sSettings>("/api/settings/k8s");

export const saveK8sSettings = (body: { context?: string; namespace?: string }) =>
  post<K8sSettings>("/api/settings/k8s", body);

// --- settings: modal ----------------------------------------------------------

export type ModalTokenSource = "env" | "syncedEnv" | "modalToml";

export interface ModalSettings {
  /** The orx-managed venv exists on disk. */
  envProvisioned: boolean;
  /** `import modal` succeeds with the resolved interpreter. */
  modalImportable: boolean;
  tokenConfigured: boolean;
  tokenSource: ModalTokenSource | null;
  /** modalImportable && tokenConfigured. */
  ready: boolean;
  error: string | null;
}

export const getModalSettings = () => get<ModalSettings>("/api/settings/modal");

/** Build the orx-managed Modal env (first run downloads the SDK, ~30–60s). */
export const provisionModal = () => post<ModalSettings>("/api/settings/modal/provision");

// --- settings: scenario ------------------------------------------------------

/** `connected` is verified against the live server, not inferred from the token
 * file: `expired` is the stored-but-dead case, and the only one where the fix is
 * to log in again. */
export type ScenarioState = "connected" | "expired" | "disconnected" | "error";

export interface ScenarioSettings {
  state: ScenarioState;
  /** Where the token lives, so the card can say it out loud. */
  authPath: string;
  /** A login started from this dashboard is still waiting for its redirect. */
  connecting: boolean;
  /** Tools the connected workspace advertises. Only set when `connected`. */
  toolCount?: number;
  /** Scenario's OAuth metadata was discoverable. Only set when `disconnected` —
   * false means a login can't succeed, which Connect alone won't fix. */
  reachable?: boolean;
  error?: string;
}

export interface ScenarioConnectStarted {
  started: boolean;
  /** Where the user has to sign in. Shown when we couldn't open a browser. */
  authorizeUrl: string;
  /** False over SSH, where the browser is on the user's machine, not the
   * server's — the card then asks them to open the URL themselves. */
  browserOpened: boolean;
}

export const getScenarioSettings = () => get<ScenarioSettings>("/api/settings/scenario");

/** Kicks off the browser login and returns at once; watch `onScenarioConnect`
 * for the outcome. */
export const connectScenario = () =>
  post<ScenarioConnectStarted>("/api/settings/scenario/connect");

export const disconnectScenario = () =>
  post<ScenarioSettings>("/api/settings/scenario/disconnect");

// --- settings: generative-ai providers ---------------------------------------

/** Blender, reached through the Blender Lab MCP server.
 *
 * Three separate booleans rather than one `ready` because their fixes differ:
 * the server can be missing, or present-but-not-runnable (a pipx venv whose
 * interpreter a Python upgrade removed), and Blender itself can simply be closed. */
export interface BlenderSettings {
  kind: "blender";
  id: string;
  name: string;
  /** Server runnable AND Blender reachable — what a tool call needs. */
  ready: boolean;
  serverFound: boolean;
  serverRunnable: boolean;
  blenderReachable: boolean;
  /** Crux has a server child running. Distinct from `serverRunnable`: a freshly
   * repaired install is runnable but needs a `crux up` restart to be running. */
  serverRunning: boolean;
  /** The `host:port` probed, so an override is visible rather than implied. */
  addonAddress: string;
  serverPath?: string;
  serverUrl?: string;
  blenderVersion?: string;
  /** Absent for an unsaved file. */
  blendFile?: string;
  objectCount?: number;
  toolCount?: number;
  serverError?: string;
  blenderError?: string;
}

/** A custom-node pack serving workflow templates whose node classes never
 *  registered — the templates look available and every submission fails. */
export interface BrokenPack {
  pack: string;
  templates: number;
  missingNodes: string[];
}

/** ComfyUI, reached through the community `comfyui-mcp` server.
 *
 * Four independent layers because the fixes differ: the MCP server can be missing
 * or unrunnable, ComfyUI itself can be down, and its custom nodes can have failed
 * to import while still advertising templates. */
export interface ComfyuiSettings {
  kind: "comfyui";
  id: string;
  name: string;
  /** MCP server runnable AND ComfyUI reachable. */
  ready: boolean;
  mcpFound: boolean;
  mcpRunnable: boolean;
  comfyReachable: boolean;
  /** Crux has an MCP child running. */
  serverRunning: boolean;
  /** Crux started ComfyUI, as opposed to adopting the user's own instance. */
  comfyManaged: boolean;
  installPath: string;
  /** ComfyUI's own web UI — the card links here. */
  dashboardUrl: string;
  mcpPath?: string;
  serverUrl?: string;
  comfyVersion?: string;
  pythonVersion?: string;
  torchVersion?: string;
  device?: string;
  vramTotal?: number;
  vramFree?: number;
  /** Registered node classes — the catalog's size, not its contents. */
  nodeClasses?: number;
  queueDepth?: number;
  /** Polled from the live server, never hard-coded. */
  toolCount?: number;
  brokenPacks?: BrokenPack[];
  mcpError?: string;
  comfyError?: string;
}

/** Fields every provider carries, so the sub-tab strip can be generic. */
export interface GenAiCommon {
  id: string;
  name: string;
  ready: boolean;
}

export type ScenarioProvider = ScenarioSettings & GenAiCommon & { kind: "scenario" };

export type GenAiProvider = ScenarioProvider | BlenderSettings | ComfyuiSettings;

export interface ComfyStartResult {
  /** We spawned it. */
  started: boolean;
  /** Something was already listening, so we took that instead. */
  adopted: boolean;
  dashboardUrl: string;
}

/** Start ComfyUI (or adopt a running one). Slow — it imports torch and scans
 *  models, so the caller should show a spinner rather than assume a quick reply. */
export const startComfyui = () => post<ComfyStartResult>("/api/settings/comfyui/start");

export const getGenAi = (refresh = false) =>
  get<{ providers: GenAiProvider[] }>(
    `/api/settings/genai${refresh ? "?refresh=1" : ""}`,
  ).then((r) => r.providers);

// --- settings: env vars / git / harnesses ------------------------------------

export interface EnvVar {
  key: string;
  maskedValue: string;
  inProcessEnv: boolean;
}

export const getEnvVars = () =>
  get<{ vars: EnvVar[] }>("/api/settings/env").then((r) => r.vars);

export const setEnvVar = (key: string, value: string) =>
  post<{ vars: EnvVar[] }>("/api/settings/env", { key, value }).then((r) => r.vars);

export const deleteEnvVar = (key: string) =>
  fetch(`/api/settings/env/${encodeURIComponent(key)}`, { method: "DELETE" })
    .then((r) => json<{ vars: EnvVar[] }>(r))
    .then((r) => r.vars);

/** Where `source` says the resolved data dir came from. `env` means the
 * `$ORX_DATA_DIR` override forces it — the UI shows the field read-only. */
export type DataDirSource = "env" | "config" | "xdg" | "default";

export interface DataDirSettings {
  current: string;
  defaultPath: string;
  isDefault: boolean;
  source: DataDirSource;
}

export const getDataDir = () => get<DataDirSettings>("/api/settings/data-dir");

export interface DataDirValidation {
  ok: boolean;
  error?: string;
  treeBytes?: number;
  freeBytes?: number;
  sameFilesystem?: boolean;
}

export const validateDataDir = (path: string) =>
  post<DataDirValidation>("/api/settings/data-dir/validate", { path });

/** Set the path without moving (onboarding / already-populated target). */
export const setDataDir = (path: string) =>
  post<DataDirSettings>("/api/settings/data-dir", { path });

/** Kick off a relocate. Resolves once the move has *started* (HTTP 202); watch
 * `onDataDirMove` (events.ts) for `progress` / `done` / `error`. Throws on the
 * 409 in-flight guard with the server's message. */
export const moveDataDir = (path: string) =>
  post<{ started: boolean }>("/api/settings/data-dir/move", { path });

export interface SshHost {
  host: string;
  hostname?: string;
  user?: string;
  port?: string;
  identityFile?: string;
  /** Most recent preflight result, persisted across restarts. */
  lastTest?: SshPreflight;
}

export const getSshHosts = () =>
  get<{ hosts: SshHost[] }>("/api/settings/ssh").then((r) => r.hosts);

export interface SshPreflight {
  reachable: boolean;
  gitFound: boolean;
  error: string | null;
  /** Unix millis. */
  testedAt: number;
}

/** Live-test a host: reachable over ssh (BatchMode) and has `git`. */
export const sshPreflight = (host: string) =>
  post<SshPreflight>("/api/settings/ssh/preflight", { host });

// --- settings: slurm ----------------------------------------------------------

export interface SlurmSettings {
  /** Default login node (an ~/.ssh/config alias); null = must pass --host. */
  host: string | null;
  /** Cluster defaults; null = the cluster decides. */
  partition: string | null;
  account: string | null;
  timeLimit: string | null;
  /** Login-node candidates, from ~/.ssh/config (same source as SSH). */
  hosts: SshHost[];
}

export const getSlurmSettings = () => get<SlurmSettings>("/api/settings/slurm");

/** Empty string clears a field back to the cluster default. */
export const saveSlurmSettings = (body: {
  host?: string;
  partition?: string;
  account?: string;
  timeLimit?: string;
}) => post<SlurmSettings>("/api/settings/slurm", body);

export interface SlurmPreflight {
  reachable: boolean;
  slurmFound: boolean;
  gitFound: boolean;
  partitions: string[];
  error: string | null;
}

/** Live-test a login node: reachable, Slurm CLI + git present, partitions. */
export const slurmPreflight = (host: string) =>
  post<SlurmPreflight>("/api/settings/slurm/preflight", { host });

// --- settings: ray ------------------------------------------------------------

export interface RaySettings {
  /** Saved Jobs / Dashboard URL; null = fall back to env / localhost. */
  address: string | null;
  /** Effective address after settings → env → default resolution. */
  resolvedAddress: string;
  /** settings | ASTROAI_RAY_JOBS_ADDRESS | RAY_DASHBOARD_URL | default */
  source: string;
}

export const getRaySettings = () => get<RaySettings>("/api/settings/ray");

/** Empty string clears the saved address (fall back to env / default). */
export const saveRaySettings = (body: { address?: string }) =>
  post<RaySettings>("/api/settings/ray", body);

export interface RayPreflight {
  reachable: boolean;
  address: string;
  rayVersion: string | null;
  error: string | null;
}

/** Live-test a Ray Jobs / Dashboard endpoint. */
export const rayPreflight = (address?: string) =>
  post<RayPreflight>("/api/settings/ray/preflight", { address: address ?? null });

// --- settings: compute targets (unified list + default) ------------------------

export type ComputeTargetId =
  | "local"
  | "hf"
  | "modal"
  | "k8s"
  | "ssh"
  | "slurm"
  | "ray"
  | "openresearch";

/** Cheap fs/env probe only — "worth trying", not "healthy". Deep health lives
 * in each backend's own settings endpoint, fetched when its row is expanded. */
export interface ComputeTargetSummary {
  id: ComputeTargetId;
  configured: boolean;
  /**
   * The readiness check couldn't run (offline, unreadable ~/.ssh), so
   * `configured` is a guess rather than an answer. Absent for backends whose
   * state is decidable locally.
   */
  unverified?: boolean;
  summary: string;
}

export interface ComputeSettings {
  defaultBackend: ComputeTargetId | null;
  defaultFlavor: string | null;
  targets: ComputeTargetSummary[];
}

export const getComputeSettings = () => get<ComputeSettings>("/api/settings/compute");

/** Set (or clear, with backend: null) the default compute target. Responds
 * with the full compute payload so the caller reconciles in one shot. */
export const setComputeDefault = (body: {
  backend: ComputeTargetId | null;
  flavor?: string | null;
}) => post<ComputeSettings>("/api/settings/compute/default", body);

export interface LocalGpu {
  name: string;
  memMib: number | null;
}

/** What `--backend local` runs on: this machine's detected hardware. */
export interface LocalMachine {
  hostname: string;
  os: string;
  arch: string;
  /** CPU brand string on macOS (e.g. "Apple M2 Pro"). */
  chip: string | null;
  cpuCount: number;
  memBytes: number | null;
  gpus: LocalGpu[];
}

export const getLocalMachine = () => get<LocalMachine>("/api/settings/local");

export interface OpenResearchSettings {
  loggedIn: boolean;
  apiUrl: string | null;
  orgs: string[];
  /**
   * Whether a registered key's private half is on THIS machine — `matched` is
   * the only state that can actually reach a box. Optional: an older `orx`
   * binary serving a newer ui omits it.
   */
  sshKeyStatus?: "matched" | "no_local_match" | "none_registered" | "unknown";
  /** The `.pub` on this machine worth registering; null if there isn't one. */
  sshKeyPath?: string | null;
  error: string | null;
}

export const getOpenResearchSettings = () =>
  get<OpenResearchSettings>("/api/settings/openresearch");

/** The experiment a top-level files folder is named for (folder == slug). */
export interface FileExperiment {
  id: string;
  slug: string;
  title?: string;
  branchName: string;
  /** The experiment's most recent run status, if it has ever run. */
  latestRunStatus?: string;
}

/** One node of the files tree: a file, or a directory with children. */
export interface FileEntry {
  name: string;
  /** Dir-relative `/`-joined path — the id for file/report/delete endpoints. */
  path: string;
  isDir: boolean;
  /** 0 for directories. */
  size: number;
  modifiedAt: number;
  /** Set when the dir holds a top-level report.md — renders as a report. */
  reportTitle?: string;
  /** Top-level dirs only: the experiment this folder corresponds to. */
  experiment?: FileExperiment;
  children?: FileEntry[];
}

/** Listing of the project's on-disk files directory. */
export interface ProjectFiles {
  dir: string;
  entries: FileEntry[];
  truncated: boolean;
}

export const getFiles = (projectId: string) =>
  get<ProjectFiles>(`/api/projects/${projectId}/files`);

export const getFileReport = (projectId: string, name: string) =>
  get<{ markdown: string }>(
    `/api/projects/${projectId}/files/report?path=${encodeURIComponent(name)}`,
  );

/** Delete a file or report folder in the files dir. */
export const deleteFile = (projectId: string, path: string) =>
  fetch(`/api/projects/${projectId}/files?path=${encodeURIComponent(path)}`, {
    method: "DELETE",
  }).then((r) => json<{ ok: boolean }>(r));

/** Raw file (images, CSVs, report figures) served by the API. */
export const fileUrl = (projectId: string, path: string) =>
  `/api/projects/${projectId}/files/file?path=${encodeURIComponent(path)}`;

/** Text body of a files-dir file (raw bytes decoded as UTF-8), or `null` when
 *  the file is missing (404). The endpoint returns bytes, not JSON, so this
 *  bypasses the `get`/`json` helpers; a 404 is a normal "not found", not an
 *  error to surface. */
export const getFilesDirFileText = (projectId: string, path: string): Promise<string | null> =>
  fetch(fileUrl(projectId, path)).then((r) => {
    if (r.status === 404) return null;
    // Bare message — the viewer prefixes "Failed to load file:" itself.
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return r.text();
  });

export interface GitSettings {
  gitVersion: string | null;
  userName: string | null;
  userEmail: string | null;
  ghInstalled: boolean;
  githubTokenSource: "env" | "stored" | "gh" | null;
}

export const getGitSettings = () => get<GitSettings>("/api/settings/git");

export const saveGitSettings = (body: { userName?: string; userEmail?: string }) =>
  post<GitSettings>("/api/settings/git", body);

/** Validate + persist a pasted GitHub token (stored in the synced env file). */
export const saveGitToken = (token: string) =>
  post<GitSettings>("/api/settings/git/token", { token });

export const removeGitToken = () =>
  fetch("/api/settings/git/token", { method: "DELETE" }).then((r) => json<GitSettings>(r));

export interface TelemetrySettings {
  /** Whether anonymous usage analytics is currently on. */
  enabled: boolean;
  /** When off, a short human reason (e.g. "--no-telemetry flag"); null when on. */
  reason: string | null;
}

export const getTelemetry = () => get<TelemetrySettings>("/api/settings/telemetry");

export const setTelemetry = (enabled: boolean) =>
  post<TelemetrySettings>("/api/settings/telemetry", { enabled });

/** Record the consent decision (agree/reject) once, when the user leaves the
 * onboarding step — fires unconditionally so opt-outs are counted too. */
export const recordTelemetryConsent = (enabled: boolean) =>
  post<{ ok: boolean }>("/api/settings/telemetry/consent", { enabled });

export type HarnessId = "claude-code" | "codex" | "opencode";

export interface HarnessModel {
  id: string;
  /**
   * Reasoning/effort choices this *specific* model accepts, led by the
   * `default` sentinel. Absent means "no list of its own" — fall back to the
   * harness-wide {@link HarnessOptions.reasoningLevels}. An empty array is
   * different: the model was checked and genuinely has no reasoning control,
   * so the picker is hidden. Use `reasoningFor` rather than reading this
   * directly.
   */
  reasoningLevels?: OptionChoice[];
  /** The catalog's own human name ("Opus", "GPT-5.6 Sol"). Absent on
   * statically-listed fallback models — derive from the id then. */
  displayName?: string;
  /** The catalog's one-line blurb. For Claude this carries the resolved
   * version ("Opus 4.8 with 1M context · …") — its aliases don't. */
  description?: string;
  /** The tier that actually runs when nothing is chosen — present only when
   * the CLI reports it (codex). When set, `reasoningLevels` has no `default`
   * sentinel and the composer preselects this concrete tier. */
  defaultReasoningLevel?: string;
}

/** Display label for a harness model: the catalog's own name when it has one,
 * else prettified from the id. */
export const harnessModelLabel = (m: HarnessModel) => m.displayName ?? modelLabel(m.id);

/** One selectable value in a composer toggle (permission mode / reasoning). */
export interface OptionChoice {
  id: string;
  label: string;
}

/**
 * The toggle vocabulary a harness supports. Empty arrays hide the control.
 *
 * `reasoningLevels` here is the harness-wide *fallback*; per-model choices ride
 * on {@link HarnessModel.reasoningLevels} and win. Resolve with `reasoningFor`.
 */
export interface HarnessOptions {
  permissionModes: OptionChoice[];
  defaultPermissionMode?: string | null;
  reasoningLevels: OptionChoice[];
  defaultReasoningLevel?: string | null;
}

/** One plan-quota bucket, normalized to percent remaining across providers. */
export interface UsageWindow {
  label: string;
  remainingPercent: number;
  /** Epoch ms when the window resets, if known. */
  resetsAtMs?: number;
}

/** Remaining-usage summary for a harness (Settings → Harnesses). */
export interface HarnessUsage {
  windows?: UsageWindow[];
  /** Epoch ms the snapshot was observed — present for captured (Codex) data. */
  observedAtMs?: number;
  /** Shown when there are no windows (e.g. OpenCode Zen). */
  note?: string;
  /** External link to manage/top-up usage (Zen dashboard). */
  manageUrl?: string;
}

/**
 * Wire id for "send no explicit effort/variant — let the CLI and its own config
 * decide". Must match `REASONING_DEFAULT_ID` in `src/local/harness/options.rs`.
 */
export const REASONING_DEFAULT_ID = "default";

/**
 * The reasoning choices to show for a harness + model, and the id to treat as
 * the default.
 *
 * Per-model metadata wins when present (Codex's per-model tiers, OpenCode's
 * `variants`); otherwise the harness-wide list applies. A model that reports an
 * empty list genuinely has no reasoning control, so the picker is hidden — this
 * is why the absent/empty distinction matters and `?? []` would be wrong.
 */
export function reasoningFor(
  harness: Harness | undefined,
  modelId: string | null | undefined,
): { choices: OptionChoice[]; defaultId: string | null } {
  const model = harness?.models.find((m) => m.id === modelId);
  const choices = model?.reasoningLevels ?? harness?.options?.reasoningLevels ?? [];
  // Preselection, in order of how much the CLI actually told us:
  //  1. the model's reported concrete default (codex) — a real tier, shown as
  //     the value that will run;
  //  2. the `default` sentinel when it's on offer — "send no override", for
  //     harnesses whose unset default isn't any fixed tier (Claude's adaptive
  //     thinking) or is unknown (opencode);
  //  3. the harness-wide default, then the first tier.
  const modelDefault = model?.defaultReasoningLevel;
  const defaultId =
    modelDefault && choices.some((c) => c.id === modelDefault)
      ? modelDefault
      : choices.some((c) => c.id === REASONING_DEFAULT_ID)
        ? REASONING_DEFAULT_ID
        : (harness?.options?.defaultReasoningLevel ?? choices[0]?.id ?? null);
  return { choices, defaultId };
}

/**
 * Keep a stored reasoning level only if the given harness+model still offers
 * it; otherwise fall back to that model's default. This is what makes switching
 * models drop an effort the new model can't accept (e.g. `ultra` off Sol onto
 * 5.5) instead of silently sending an invalid value.
 */
export function reconcileReasoning(
  harness: Harness | undefined,
  modelId: string | null | undefined,
  current: string | null,
): string | null {
  // Harness not resolved yet — detection is async, and it can also fail. "We
  // don't know what this model offers" is not "this model offers nothing", so
  // leave the stored level alone; resetting it here would let a send that races
  // the harness fetch overwrite a deliberate choice.
  if (!harness) return current;
  const { choices, defaultId } = reasoningFor(harness, modelId);
  // A model with no reasoning control: the picker is hidden, so the user can no
  // longer clear a level themselves. Return the sentinel rather than `null` —
  // `null` reads as "no override supplied" and leaves whatever the session row
  // already holds in place, which would keep sending a stale level the model
  // never offered.
  if (choices.length === 0) return REASONING_DEFAULT_ID;
  return current && choices.some((c) => c.id === current) ? current : defaultId;
}

export interface Harness {
  id: HarnessId;
  name: string;
  installed: boolean;
  binPath?: string;
  version?: string;
  authenticated: boolean;
  authState: "ready" | "needsLogin" | "unknown" | "unsupported";
  authMethod?: "oauth" | "apiKey";
  account?: string;
  org?: string;
  plan?: string;
  agentReady: boolean;
  agentNote?: string;
  models: HarnessModel[];
  options: HarnessOptions;
  usage?: HarnessUsage;
}

export const getHarnesses = (refresh = false, retryRejected = false) => {
  const params = new URLSearchParams();
  if (refresh) params.set("refresh", "1");
  if (retryRejected) params.set("retry", "1");
  const query = params.size > 0 ? `?${params.toString()}` : "";
  return get<{ harnesses: Harness[] }>(`/api/harnesses${query}`).then((r) => r.harnesses);
};

/** Slash-skill offered in the composer's `/` dropdown; expanded server-side. */
export interface SkillInfo {
  name: string;
  description: string;
  argHint: string;
}

export const getSkills = () => get<{ skills: SkillInfo[] }>("/api/skills").then((r) => r.skills);

/** "openai/gpt-5.5" → "GPT 5.5", "anthropic/claude-opus-4-8" → "Opus 4.8". */
export function modelLabel(id: string): string {
  const last = (id.split("/").pop() ?? id).replace(/^~/, "").replace(/^claude-/, "");
  const words: string[] = [];
  const nums: string[] = [];
  for (const part of last.split("-")) {
    if (/^\d+(\.\d+)?$/.test(part)) {
      nums.push(part);
    } else {
      if (nums.length) words.push(nums.splice(0).join("."));
      words.push(part === "gpt" ? "GPT" : part.charAt(0).toUpperCase() + part.slice(1));
    }
  }
  if (nums.length) words.push(nums.join("."));
  return words.join(" ");
}

// --- chat (unified harness sessions) ------------------------------------------

export interface ChatToolState {
  status: "running" | "completed" | "error";
  input?: { command?: string; filePath?: string; description?: string; [k: string]: unknown };
  output?: string;
  error?: string;
  title?: string;
}

export interface ChatQuestionOption {
  label: string;
  description?: string;
}

/** An interactive request the user acts on before the harness continues. */
export interface ChatPrompt {
  kind: "plan" | "permission" | "question";
  resolved: boolean;
  plan?: string;
  /** plan: card synthesized from the turn's final text (no ExitPlanMode call). */
  synthesized?: boolean;
  tool?: string;
  toolInput?: Record<string, unknown>;
  question?: string;
  header?: string;
  options?: ChatQuestionOption[];
  multiSelect?: boolean;
  /** Answer echo, stamped on resolve: chosen labels (questions), whether the
   * card was approved (plan/permission), and any freeform note. Absent on
   * cards resolved without an answer (stale-card cleanup). */
  answers?: string[];
  approved?: boolean;
  note?: string;
  /** Backend resume routing id. Presence marks a HELD mid-turn card (the
   * turn is blocked open waiting on this answer); absent on end-turn cards. */
  nativeId?: string;
}

export interface ChatPart {
  id: string;
  type: string; // text | reasoning | tool | prompt
  text?: string;
  tool?: string;
  state?: ChatToolState;
  prompt?: ChatPrompt;
  /** Nested transcript of a sub-agent this part spawned (Codex `subagent`
   * tool). Streams live and recurses for sub-agents that spawn their own. */
  children?: ChatPart[];
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  parts: ChatPart[];
  createdAt: number;
}

/** How much of the model's context window a session has used, measured off the
 * most recent API request (latest wins, not cumulative). `contextWindow` is
 * absent when the harness doesn't report one. */
export interface ContextUsage {
  usedTokens: number;
  contextWindow?: number;
}

export interface ChatSession {
  id: string;
  projectId: string;
  harness: HarnessId;
  title: string | null;
  /** Who wrote `title`: `"fallback"` (first-line placeholder), `"generated"`
   * (harness auto-title), `"user"` (rename). Null on legacy sessions. */
  titleSource?: string | null;
  /** The model the user pinned for this session; null = harness default. */
  model: string | null;
  /** The model the harness actually resolved and ran, observed at turn time.
   *  Display only — never send it back as a pin. Fills in the blank when `model`
   *  is null, which is otherwise unanswerable: OpenCode picks its default from
   *  the authenticated providers and reports it only at runtime. */
  effectiveModel?: string | null;
  permissionMode: string | null;
  reasoningLevel: string | null;
  /** The session's own persona wire id; null = inherit the project's persona. */
  persona?: string | null;
  /** The orchestrator session that spawned this one; null = top-level. */
  parentSessionId?: string | null;
  /** Hidden from the default Recents list, but fully intact and resumable. */
  archived: boolean;
  createdAt: number;
  updatedAt: number;
  busy: boolean;
  contextUsage?: ContextUsage;
}

export const listChatSessions = (projectId: string) =>
  get<{ sessions: ChatSession[] }>(
    `/api/chat/sessions?projectId=${encodeURIComponent(projectId)}`,
  ).then((r) => r.sessions);

/** Per-session (and per-turn) composer selections beyond the harness itself. */
export interface TurnOptions {
  model?: string | null;
  permissionMode?: string | null;
  reasoningLevel?: string | null;
  persona?: string | null;
}

export const createChatSession = (
  projectId: string,
  harness: HarnessId,
  opts: TurnOptions = {},
) =>
  post<{ session: ChatSession }>("/api/chat/sessions", { projectId, harness, ...opts }).then(
    (r) => r.session,
  );

export const deleteChatSession = (sessionId: string) =>
  fetch(`/api/chat/sessions/${sessionId}`, { method: "DELETE" }).then((r) =>
    json<{ ok: boolean }>(r),
  );

/** Archive/unarchive a session (archived chats stay resumable). */
export const setChatSessionArchived = (sessionId: string, archived: boolean) =>
  patch<{ session: ChatSession }>(`/api/chat/sessions/${sessionId}`, { archived }).then(
    (r) => r.session,
  );

/** Rename a session. The title is trimmed server-side; empty titles are rejected. */
export const renameChatSession = (sessionId: string, title: string) =>
  patch<{ session: ChatSession }>(`/api/chat/sessions/${sessionId}`, { title }).then(
    (r) => r.session,
  );

export const getChatMessages = (sessionId: string) =>
  get<{ messages: ChatMessage[] }>(`/api/chat/sessions/${sessionId}/messages`).then(
    (r) => r.messages,
  );

/** Pasted image riding a chat message. */
export interface ChatImageAttachment {
  mediaType: string;
  dataBase64: string;
}

/** Image parts store a server-minted file name; this is where it's served. */
export const chatAttachmentUrl = (name: string) =>
  `/api/chat/attachments/${encodeURIComponent(name)}`;

/** Returns immediately; the turn streams over /api/events (chat.* events). */
export const sendChatMessage = (
  sessionId: string,
  text: string,
  opts: TurnOptions = {},
  images?: ChatImageAttachment[],
) =>
  post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/message`, {
    text,
    model: opts.model,
    permissionMode: opts.permissionMode,
    reasoningLevel: opts.reasoningLevel,
    images,
  });

export const interruptChat = (sessionId: string) =>
  post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/interrupt`);

/** Answer an interactive prompt (plan / permission / question) on a session. */
export interface PromptAnswer {
  promptId: string;
  approve?: boolean;
  /** Permission mode to resume under (plan/permission approval). */
  resumeMode?: string;
  /** Chosen option labels (questions). */
  answers?: string[];
  note?: string;
}

export const respondChat = (sessionId: string, answer: PromptAnswer) =>
  post<{ ok: boolean }>(`/api/chat/sessions/${sessionId}/respond`, answer);

// --- helpers shared across views --------------------------------------------

export function statusColor(status: string): string {
  switch (status) {
    case "done":
      return "var(--green)";
    case "running":
      return "var(--teal)";
    case "starting":
      return "var(--amber)";
    case "failed":
      return "var(--red)";
    case "cancelled":
      return "var(--muted)";
    default:
      return "var(--muted)";
  }
}

export function timeAgo(ms: number): string {
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
}

/** "42s" / "18m" / "2h 28m" / "1d 4h" — an elapsed duration, not a timestamp. */
export function fmtDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

/** Compact byte size, e.g. "512 B", "2.0 KB", "5.3 MB". Mirrors the backend's
 * `store::human_bytes`. */
export function fmtBytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return u === 0 ? `${n} B` : `${v.toFixed(1)} ${units[u]}`;
}

/** Compact token count, e.g. 62300 → "62k", 1_200_000 → "1.2M", 940 → "940". */
export function fmtTokens(n: number): string {
  if (n < 1000) return `${Math.round(n)}`;
  // One decimal, dropped when it's .0 ("31.4k", "200k", "1M"). The k branch
  // stops where toFixed(1) would round to "1000.0" (e.g. 999_960 → "1M").
  if (n < 999_950) return `${trimZero((n / 1000).toFixed(1))}k`;
  return `${trimZero((n / 1_000_000).toFixed(1))}M`;
}

function trimZero(s: string): string {
  return s.endsWith(".0") ? s.slice(0, -2) : s;
}

export function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 10)}…` : id;
}

/** The backend kind from a run's `backend` descriptor ("modal_job", "hf_job", …). */
export function backendKind(backend: Run["backend"]): string {
  if (!backend) return "";
  if (typeof backend.kind === "string") return backend.kind;
  if (typeof backend.type === "string") return backend.type;
  return "";
}

/** The flavor / manifest / host that qualifies a backend, if any. k8s runs
 *  carry a manifest path instead of a flavor; ssh a host in `namespace`. */
export function backendDetail(backend: Run["backend"]): string {
  if (!backend) return "";
  if (typeof backend.flavor === "string" && backend.flavor) return backend.flavor;
  if (typeof backend.manifest === "string" && backend.manifest) return backend.manifest;
  // Ray's namespace is the whole Jobs URL — too long for a badge.
  if (backendKind(backend) === "ray_job") return "";
  if (typeof backend.namespace === "string" && backend.namespace) return backend.namespace;
  return "";
}
