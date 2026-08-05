//! The bundled idea evaluator — the deterministic half of an idea evaluation.
//!
//! The analyst codes the 35 market-fit signals itself (that's judgment, and it
//! runs on whatever harness the session uses); this evaluator does only the
//! arithmetic on that coding: per-KPI alignment fractions, anti-pattern and
//! table-stakes counts, top comparables, and the resolved-signal confidence
//! share. It is Evaluation v2 — transparent fractions over skill outputs, no
//! invented weights, no thresholds, no verdicts.
//!
//! **Why it ships in the binary.** The evaluator used to be expected at
//! `tools/idea-evaluator/` inside each project repo, which meant a blank game
//! repo (every repo the idea pipeline creates) had no rubric and no math: the
//! analyst's first step died on `Cannot find module`. Bundling it here gives one
//! source of truth that can't drift per project, resolves offline, and keeps
//! ~330KB of generated reference data out of every game repo.
//!
//! [`ensure`] materializes the tree under `<data dir>/idea-evaluator/` and
//! stamps it with this build's version, so an upgraded binary refreshes the
//! files rather than serving a stale copy. `orx evaluator path` prints that
//! directory (ensuring it first) — the skill resolves the evaluator that way
//! instead of hardcoding a path, and sim runs get it via `ORX_EVALUATOR_DIR`.

use std::path::PathBuf;

use crate::error::{anyhow, Result};
use crate::store::data_dir;

/// Env var carrying the materialized evaluator dir into harness children and
/// sim runs, so a run command can invoke it without shelling out to `orx`.
pub const EVALUATOR_DIR_ENV: &str = "ORX_EVALUATOR_DIR";

/// The evaluator tree, embedded at build time. Vendored from
/// `polats/foundry-idea-demo:tools/idea-evaluator/`; keep in sync with the
/// on-disk `agent-skills/orx-evaluate/evaluator/` copy (which is also what
/// `node test-core.mjs` runs against).
const EVALUATOR_FILES: &[(&str, &str)] = &[
    (
        "evaluate.mjs",
        include_str!("../../agent-skills/orx-evaluate/evaluator/evaluate.mjs"),
    ),
    (
        "eval-core.mjs",
        include_str!("../../agent-skills/orx-evaluate/evaluator/eval-core.mjs"),
    ),
    (
        "eval-data.mjs",
        include_str!("../../agent-skills/orx-evaluate/evaluator/eval-data.mjs"),
    ),
    (
        "llm.mjs",
        include_str!("../../agent-skills/orx-evaluate/evaluator/llm.mjs"),
    ),
    (
        "test-core.mjs",
        include_str!("../../agent-skills/orx-evaluate/evaluator/test-core.mjs"),
    ),
    (
        "EVALUATING.md",
        include_str!("../../agent-skills/orx-evaluate/evaluator/EVALUATING.md"),
    ),
    (
        "fixtures/skybloom.coding.json",
        include_str!("../../agent-skills/orx-evaluate/evaluator/fixtures/skybloom.coding.json"),
    ),
];

/// `<data dir>/idea-evaluator/` — where [`ensure`] materializes the tree.
pub fn dir() -> PathBuf {
    data_dir().join("idea-evaluator")
}

/// Write the bundled evaluator out if it's missing or was written by a different
/// build, and return its directory. Idempotent and cheap on the common path (one
/// stamp read), so callers can call it freely — startup, every harness turn, and
/// each sim run all do.
pub fn ensure() -> Result<PathBuf> {
    let root = dir();
    let stamp = root.join(".version");
    let want = env!("CARGO_PKG_VERSION");
    // A matching stamp means this build already wrote these bytes. Any mismatch
    // (older build, hand-edited copy, partial write) rewrites every file.
    if std::fs::read_to_string(&stamp).is_ok_and(|got| got.trim() == want) {
        return Ok(root);
    }
    for (rel, content) in EVALUATOR_FILES {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
        }
        std::fs::write(&path, content)
            .map_err(|e| anyhow!("Could not write {}: {}", path.display(), e))?;
    }
    // Stamp last: a crash mid-write leaves no stamp, so the next call retries
    // rather than trusting a half-written tree.
    std::fs::write(&stamp, want)
        .map_err(|e| anyhow!("Could not write {}: {}", stamp.display(), e))?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded tree must carry the three files the evaluator can't run
    /// without — the entry point, the arithmetic, and the reference data whose
    /// game profiles the comparables come from.
    #[test]
    fn bundles_the_files_the_evaluator_needs() {
        for want in ["evaluate.mjs", "eval-core.mjs", "eval-data.mjs"] {
            let found = EVALUATOR_FILES.iter().find(|(rel, _)| *rel == want);
            let (_, content) = found.unwrap_or_else(|| panic!("{want} is not bundled"));
            assert!(!content.trim().is_empty(), "{want} is empty");
        }
    }

    /// The reference data has to actually contain the 35 signal definitions and
    /// the per-KPI classification tables. A truncated or wrong-file vendoring
    /// would still compile and still write files, then fail inside node at the
    /// exact step the analyst can't recover from.
    #[test]
    fn reference_data_carries_the_rubric() {
        let (_, data) = EVALUATOR_FILES
            .iter()
            .find(|(rel, _)| *rel == "eval-data.mjs")
            .expect("eval-data.mjs bundled");
        assert!(data.contains("SIGNAL_DEFINITIONS_MD"), "signal definitions");
        assert!(data.contains("ARCHETYPE_REFERENCE_MD"), "archetype reference");
        assert!(data.contains("GAME_SIGNAL_PROFILES"), "game profiles");
        // Spot-check both ends of the signal list — the first and the one the
        // coding rules call out as always-Unresolved.
        assert!(data.contains("short_session"));
        assert!(data.contains("game_age_gte_2yr"));
    }

    #[test]
    fn ensure_writes_the_tree_and_restamps_a_stale_copy() {
        let tmp = std::env::temp_dir().join(format!("orx-evaluator-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        // Point the data dir at the temp root for this process only.
        std::env::set_var("ORX_DATA_DIR", &tmp);

        let root = ensure().unwrap();
        assert!(root.join("evaluate.mjs").is_file());
        assert!(root.join("fixtures/skybloom.coding.json").is_file());
        assert_eq!(
            std::fs::read_to_string(root.join(".version")).unwrap(),
            env!("CARGO_PKG_VERSION")
        );

        // A stale stamp rewrites the tree: emptying a file and downgrading the
        // stamp must heal on the next call.
        std::fs::write(root.join("evaluate.mjs"), "").unwrap();
        std::fs::write(root.join(".version"), "0.0.0").unwrap();
        ensure().unwrap();
        assert!(
            !std::fs::read_to_string(root.join("evaluate.mjs"))
                .unwrap()
                .is_empty(),
            "stale copy was not refreshed"
        );

        std::env::remove_var("ORX_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
