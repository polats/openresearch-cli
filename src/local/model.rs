//! Wire-friendly local-mode entities — the same camelCase shapes the `orx up`
//! HTTP API serves. Row conversions live here beside the structs; the SQL
//! (matching column order) lives in `store.rs`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalProject {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub github_owner: String,
    pub github_repo: String,
    /// Fork point for baseline roots and the clone's default checkout — not
    /// where any experiment lives (legacy roots predating per-baseline
    /// branches may still ride it).
    pub baseline_branch: String,
    /// Local clone path (`~/.cache/openresearch/repos/<owner>/<repo>`).
    pub repo_path: String,
    pub run_command: Option<String>,
    /// arXiv id the project starts from (versionless, e.g. `2401.12345`).
    pub paper_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Build command for playable experiment builds (default `npm run build`).
    pub play_command: Option<String>,
    /// Dir (repo-relative) the play build outputs, served at /play (default `dist`).
    pub play_dir: Option<String>,
    /// Agent persona wire id (`research` | `game-designer`); NULL = research.
    pub persona: Option<String>,
    /// Which automatic `[orx]` chat prompts fire for this project. NULL (and
    /// every unset field) = off — the agent is only prompted when the user
    /// opts in via the Persona tab.
    pub auto_prompts: Option<AutoPrompts>,
}

/// Per-project switches for the run-watcher's automatic chat prompts
/// (`chat::watch_runs`). All default **off**: unsolicited agent turns proved
/// intrusive during playtests. Stored as JSON in `local_projects.auto_prompts`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoPrompts {
    /// A play session ended → ask the user how it felt, record the verdict.
    pub play_session: bool,
    /// A play build failed → tell the agent to fix the branch.
    pub play_build_failed: bool,
    /// A job/sim run completed → reconcile and continue the loop.
    pub run_completed: bool,
}

impl LocalProject {
    /// Column order must match `store::PROJECT_COLS`.
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> std::result::Result<Self, rusqlite::Error> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            slug: row.get(2)?,
            github_owner: row.get(3)?,
            github_repo: row.get(4)?,
            baseline_branch: row.get(5)?,
            repo_path: row.get(6)?,
            run_command: row.get(7)?,
            paper_id: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
            play_command: row.get(11)?,
            play_dir: row.get(12)?,
            persona: row.get(13)?,
            auto_prompts: row
                .get::<_, Option<String>>(14)?
                .and_then(|s| serde_json::from_str(&s).ok()),
        })
    }

    /// The agent persona this project runs under. Defaults to research; an
    /// unknown stored value also falls back rather than breaking sessions.
    pub fn persona(&self) -> super::agent_skills::Persona {
        super::agent_skills::Persona::parse(self.persona.as_deref()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalExperiment {
    pub id: String,
    pub project_id: String,
    /// NULL = baseline/root.
    pub parent_experiment_id: Option<String>,
    pub slug: String,
    /// `orx/<slug>` (legacy baselines ride the project's baseline branch).
    pub branch_name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub run_command: String,
    pub agent_status: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Standing human verdict: keep | kill | iterate. Set explicitly, never
    /// derived from run verdicts (see the Phase 1 spec).
    pub verdict: Option<String>,
    pub verdict_notes: Option<String>,
    pub verdict_at: Option<i64>,
    /// The playable's entry page under /play/<id>/ (path + optional query,
    /// e.g. `gambit-slots.html?x=1`). None = the build's index.html.
    pub play_entry: Option<String>,
    /// Second parent for merge nodes: the experiment whose branch was merged
    /// into this one at creation (`create-experiment --merge`). Drawn as a
    /// dashed extra edge in the tree.
    pub merge_parent_experiment_id: Option<String>,
}

impl LocalExperiment {
    /// Column order must match `store::EXPERIMENT_COLS`.
    pub(crate) fn from_row(row: &rusqlite::Row<'_>) -> std::result::Result<Self, rusqlite::Error> {
        Ok(Self {
            id: row.get(0)?,
            project_id: row.get(1)?,
            parent_experiment_id: row.get(2)?,
            slug: row.get(3)?,
            branch_name: row.get(4)?,
            title: row.get(5)?,
            description: row.get(6)?,
            run_command: row.get(7)?,
            agent_status: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
            verdict: row.get(11)?,
            verdict_notes: row.get(12)?,
            verdict_at: row.get(13)?,
            play_entry: row.get(14)?,
            merge_parent_experiment_id: row.get(15)?,
        })
    }

    /// Display name: title when set, slug otherwise.
    pub fn display_name(&self) -> &str {
        match self.title.as_deref() {
            Some(t) if !t.trim().is_empty() => t,
            _ => &self.slug,
        }
    }
}
