//! `crux scenario connect | status | disconnect | tools | call` — the Scenario
//! MCP bridge as a CLI surface.
//!
//! `connect` is the one interactive entry point on the machine; everything else
//! is the silent path. `tools` and `call` exist because Scenario advertises "60+
//! tools" without publishing their names — they're how you discover what the
//! connected workspace actually offers, and they stay useful afterwards for
//! poking at a tool's shape before wiring it into a workflow node.

use crate::error::{anyhow, Result};
use crate::local::scenario;

pub async fn run(args: crate::ScenarioArgs) -> Result<()> {
    match args.command {
        crate::ScenarioCommand::Connect => connect().await,
        crate::ScenarioCommand::Status => status().await,
        crate::ScenarioCommand::Disconnect => disconnect(),
        crate::ScenarioCommand::Tools { json, filter } => tools(json, filter).await,
        crate::ScenarioCommand::Call { name, args } => call(&name, args.as_deref()).await,
    }
}

async fn connect() -> Result<()> {
    if scenario::is_connected() {
        println!("Already connected. Re-running the login will replace the stored token.");
    }
    let scopes = scenario::auth::connect().await?;
    println!("\u{2713} Connected to Scenario.");
    if !scopes.is_empty() {
        println!("  Granted scopes: {}", scopes.join(" "));
    }
    println!(
        "  Token stored at {} (mode 0600).",
        scenario::auth::auth_path().display()
    );
    Ok(())
}

/// Reports both halves of "connected": whether a token is stored, and whether it
/// still works. They diverge — a stored token whose refresh has been redeemed
/// elsewhere looks present but fails on use — and only the second answer tells
/// you whether a workflow run will succeed, so `status` pays for a live call.
async fn status() -> Result<()> {
    if !scenario::is_connected() {
        println!("Scenario: not connected");
        // Report discovery too: "not connected" and "can't connect" look the
        // same from here, and only the second is a problem to fix.
        match scenario::auth::discover().await {
            Ok(d) => {
                println!("  OAuth endpoints discovered ({}):", d.source);
                println!("    authorize:    {}", d.authorize);
                println!("    token:        {}", d.token);
                match d.registration.as_deref() {
                    Some(url) => println!("    registration: {url}"),
                    // Without it there is no client to register, and Crux
                    // registers dynamically rather than shipping a client id.
                    None => println!("    registration: (none advertised — login cannot work)"),
                }
            }
            Err(e) => println!("  Cannot discover Scenario's OAuth endpoints: {e}"),
        }
        println!("  Run `crux scenario connect` to sign in with your Scenario account.");
        return Ok(());
    }
    println!(
        "Scenario: login stored at {}",
        scenario::auth::auth_path().display()
    );
    print!("  Checking it still works… ");
    use std::io::Write;
    let _ = std::io::stdout().flush();

    match scenario::list_tools().await {
        Ok(tools) => {
            println!("ok");
            println!("  {} tools available.", tools.len());
        }
        Err(scenario::ScenarioError::NotConnected) => {
            println!("expired");
            println!("  The stored login no longer works — run `crux scenario connect` again.");
        }
        Err(e) => {
            println!("failed");
            println!("  {e}");
        }
    }
    Ok(())
}

fn disconnect() -> Result<()> {
    if !scenario::is_connected() {
        println!("Scenario: already not connected.");
        return Ok(());
    }
    scenario::auth::forget()?;
    println!("\u{2713} Forgot the stored Scenario login.");
    Ok(())
}

async fn tools(json: bool, filter: Option<String>) -> Result<()> {
    let mut tools = scenario::list_tools().await?;
    if let Some(filter) = filter.as_deref() {
        let needle = filter.to_ascii_lowercase();
        tools.retain(|t| {
            t.name.to_ascii_lowercase().contains(&needle)
                || t.description
                    .as_deref()
                    .is_some_and(|d| d.to_ascii_lowercase().contains(&needle))
        });
    }
    tools.sort_by(|a, b| a.name.cmp(&b.name));

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&tools)
                .map_err(|e| anyhow!("cannot serialize the tool list: {e}"))?
        );
        return Ok(());
    }
    if tools.is_empty() {
        println!("No tools matched.");
        return Ok(());
    }
    for tool in &tools {
        // One line per tool: the full input schema is what `--json` is for.
        let summary = tool
            .description
            .as_deref()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("");
        println!("{:<34} {}", tool.name, summary);
    }
    println!("\n{} tools.", tools.len());
    Ok(())
}

async fn call(name: &str, args: Option<&str>) -> Result<()> {
    let args = match args {
        None => serde_json::Value::Null,
        Some(raw) => serde_json::from_str(raw)
            .map_err(|e| anyhow!("arguments must be a JSON object: {e}"))?,
    };
    // Print the raw result rather than a typed shape: this is a discovery tool,
    // and the point is to see exactly what a tool returns before modelling it.
    let result = scenario::call_raw(name, args).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result)
            .map_err(|e| anyhow!("cannot serialize the result: {e}"))?
    );
    Ok(())
}
