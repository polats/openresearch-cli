//! Kimodo — NVIDIA's kinematic motion diffusion, used to generate looping motion
//! for a rigged character.
//!
//! Status only, no managed process. Kimodo runs as a Python job against a local
//! checkout and a Hugging Face checkpoint; there is no long-lived service to
//! supervise, so a harness would be a lifecycle around something with no lifecycle
//! (the same reasoning that removed `sprite-mcp` and `unirig-mcp` from the plan).
//!
//! The card answers two questions before a run is attempted: is the code there, and
//! is a checkpoint downloaded. Licence terms are deliberately out of scope — reading
//! them is the user's call, not something a status card should adjudicate.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Where the checkpoints land when pulled from Hugging Face.
fn hf_hub() -> PathBuf {
    std::env::var("HF_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join(".cache/huggingface"))
        .join("hub")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default()
}

/// One downloaded checkpoint.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KimodoCheckpoint {
    /// Hugging Face repo id, e.g. `nvidia/Kimodo-SMPLX-RP-v1`.
    pub repo: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KimodoStatus {
    /// A Kimodo checkout exists.
    pub repo_found: bool,
    /// …and it has a Python environment to run in. Separate because a checkout
    /// without a venv needs `uv sync`, not a re-clone.
    pub venv_found: bool,
    /// At least one checkpoint is downloaded.
    pub model_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python_path: Option<String>,
    pub checkpoints: Vec<KimodoCheckpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl KimodoStatus {
    /// Everything a motion run needs present on disk.
    pub fn ready(&self) -> bool {
        self.repo_found && self.model_present
    }
}

/// Candidate checkout locations, override first.
fn repo_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("KIMODO_HOME") {
        out.push(PathBuf::from(p));
    }
    out.push(dirs_home().join("projects/kimodo"));
    out
}

/// A venv interpreter inside the checkout, if one was created.
fn find_python(repo: &Path) -> Option<PathBuf> {
    for rel in [".venv/bin/python", "venv/bin/python"] {
        let p = repo.join(rel);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Downloaded `nvidia/Kimodo-*` checkpoints, newest-looking first.
fn scan_checkpoints() -> Vec<KimodoCheckpoint> {
    let hub = hf_hub();
    let Ok(entries) = std::fs::read_dir(&hub) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // `models--nvidia--Kimodo-SMPLX-RP-v1` → `nvidia/Kimodo-SMPLX-RP-v1`
        let Some(rest) = name.strip_prefix("models--") else {
            continue;
        };
        let repo = rest.replace("--", "/");
        if !repo.to_lowercase().contains("kimodo") {
            continue;
        }
        out.push(KimodoCheckpoint { repo });
    }
    out.sort_by(|a, b| a.repo.cmp(&b.repo));
    out
}

pub async fn detect() -> KimodoStatus {
    let mut status = KimodoStatus::default();

    if let Some(repo) = repo_candidates().into_iter().find(|p| p.is_dir()) {
        status.repo_found = true;
        status.repo_path = Some(repo.display().to_string());
        if let Some(py) = find_python(&repo) {
            status.venv_found = true;
            status.python_path = Some(py.display().to_string());
        }
    } else {
        status.error = Some(
            "no Kimodo checkout found — clone it, or set KIMODO_HOME to an existing one."
                .to_string(),
        );
    }

    status.checkpoints = scan_checkpoints();
    status.model_present = !status.checkpoints.is_empty();
    if !status.model_present && status.error.is_none() {
        status.error =
            Some("no nvidia/Kimodo-* checkpoint downloaded into the Hugging Face cache.".into());
    }
    status
}
