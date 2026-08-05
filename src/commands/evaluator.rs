//! `orx evaluator path` — resolve the bundled idea evaluator.
//!
//! The analyst's skill needs a path it can hand to `node`, and the evaluator
//! ships inside this binary (see [`crate::local::evaluator`]) rather than in the
//! project repo. This materializes it if needed and prints the directory, so the
//! skill stays a one-liner — `node "$(orx evaluator path)/evaluate.mjs" …` —
//! that works regardless of whether the harness passed `ORX_EVALUATOR_DIR`
//! through to the agent's shell.

use crate::error::{anyhow, Result};
use crate::EvaluatorArgs;

pub async fn run(args: EvaluatorArgs) -> Result<()> {
    match args.what.as_deref().unwrap_or("path") {
        "path" => {
            let dir = crate::local::evaluator::ensure()?;
            println!("{}", dir.display());
            Ok(())
        }
        other => Err(anyhow!(
            "unknown evaluator subcommand '{other}' — the only verb is `path`"
        )),
    }
}
