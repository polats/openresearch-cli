//! `orx agent` — subagent dispatch proposals.
//!
//! The orchestrator agent SUGGESTS a subagent (`orx agent suggest`), writing a
//! pending proposal to the local store. The human reviews it in the dashboard,
//! optionally overrides the persona/harness/model, and approves — which spawns
//! a chat session (server-side). This CLI covers the agent-facing verbs
//! (suggest / list / status); approval + spawn live in the `orx up` server.

use crate::error::{anyhow, Result};
use crate::store::{now_ms, Store, StoredAgentProposal};
use crate::{AgentArgs, AgentCommand};

pub async fn run(args: AgentArgs) -> Result<()> {
    let store = Store::open()?;
    match args.command {
        AgentCommand::Suggest {
            project_id,
            task,
            persona,
            harness,
            model,
            parent,
            from_session,
            why,
        } => {
            if store.get_local_project(&project_id)?.is_none() {
                return Err(anyhow!(
                    "no local project {project_id} (agent dispatch is local-mode only)"
                ));
            }
            // Validate the suggested persona wire id up front, if given.
            if let Some(p) = persona.as_deref() {
                crate::local::agent_skills::Persona::parse(Some(p)).map_err(|e| anyhow!(e))?;
            }
            let id = format!("prop_{}", uuid::Uuid::new_v4());
            let now = now_ms();
            let prop = StoredAgentProposal {
                id: id.clone(),
                project_id,
                parent_experiment_id: parent,
                persona,
                harness,
                model,
                task,
                why,
                status: "pending".into(),
                session_id: None,
                parent_session_id: from_session,
                created_at: now,
                updated_at: now,
            };
            store.create_agent_proposal(&prop)?;
            println!("Suggested subagent {id} — pending the human's approval.");
            if let Some(p) = &prop.persona {
                println!("  persona: {p}");
            }
            if let Some(h) = &prop.harness {
                println!("  harness: {h}");
            }
            if let Some(m) = &prop.model {
                println!("  model:   {m}");
            }
            println!("  task:    {}", prop.task);
            Ok(())
        }

        AgentCommand::List { project_id } => {
            let props = store.list_agent_proposals(&project_id)?;
            if props.is_empty() {
                println!("No proposals.");
                return Ok(());
            }
            for p in props {
                println!(
                    "{}  {:9}  {:14}  {}",
                    p.id,
                    p.status,
                    p.persona.as_deref().unwrap_or("-"),
                    truncate(&p.task, 60),
                );
            }
            Ok(())
        }

        AgentCommand::Status { proposal_id } => {
            let p = store
                .get_agent_proposal(&proposal_id)?
                .ok_or_else(|| anyhow!("no proposal {proposal_id}"))?;
            println!("id:      {}", p.id);
            println!("status:  {}", p.status);
            println!("project: {}", p.project_id);
            for (label, val) in [
                ("parent", &p.parent_experiment_id),
                ("from-session", &p.parent_session_id),
                ("persona", &p.persona),
                ("harness", &p.harness),
                ("model", &p.model),
                ("why", &p.why),
                ("session", &p.session_id),
            ] {
                if let Some(v) = val {
                    println!("{label}: {v}");
                }
            }
            println!("task:    {}", p.task);
            Ok(())
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() <= n {
        one_line
    } else {
        format!("{}…", one_line.chars().take(n).collect::<String>())
    }
}
