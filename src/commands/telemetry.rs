//! `orx telemetry status | on | off | context | key` — inspect and control
//! anonymous usage analytics. The discoverable, persistent opt-out (also
//! toggleable from the `orx up` onboarding step), the machine-context tag used
//! by fleet provisioning to mark automated installs, and this fork's PostHog
//! key — which Crux ships empty, so analytics are off until you set one.

use crate::error::{anyhow, Result};
use crate::telemetry;

pub async fn run(args: crate::TelemetryArgs) -> Result<()> {
    match args.command {
        crate::TelemetryCommand::Status => status(),
        crate::TelemetryCommand::On => set_enabled(true).await,
        crate::TelemetryCommand::Off => set_enabled(false).await,
        crate::TelemetryCommand::Context { value, clear } => context(value, clear),
        crate::TelemetryCommand::Key { value, clear } => key(value, clear),
    }
}

/// Show or set this fork's PostHog project key. Crux deliberately ships none —
/// inheriting upstream's key would report every install into upstream's
/// analytics — so this (or `CRUX_POSTHOG_KEY`) is what turns analytics on.
fn key(value: Option<String>, clear: bool) -> Result<()> {
    if clear {
        telemetry::set_posthog_key(None)
            .map_err(|e| anyhow!("Could not clear PostHog key: {e}"))?;
        println!("\u{2713} PostHog key cleared (analytics are off).");
        return Ok(());
    }
    let Some(value) = value else {
        match telemetry::configured_posthog_key() {
            Some(k) => println!("PostHog key: {k}"),
            None => println!("PostHog key: (none — analytics are off)"),
        }
        return Ok(());
    };
    let value = value.trim();
    // A `phx_` personal key can read data and change project settings; shipping
    // one in a client would be a credential leak, so refuse it outright.
    if !value.starts_with("phc_") {
        return Err(anyhow!(
            "expected a public, write-only project key starting with `phc_` (never a personal `phx_` key)"
        ));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(anyhow!("a PostHog key cannot contain whitespace"));
    }
    telemetry::set_posthog_key(Some(value.to_string()))
        .map_err(|e| anyhow!("Could not save PostHog key: {e}"))?;
    println!("\u{2713} PostHog key saved. Analytics will report to your project.");
    println!("  Turn it off any time with `orx telemetry off` or `orx telemetry key --clear`.");
    Ok(())
}

/// Show or set the machine context tag (`install_kind` on every event). Values
/// are coarse machine-class labels ("cloud-agent"), never anything identifying.
fn context(value: Option<String>, clear: bool) -> Result<()> {
    if clear {
        telemetry::set_machine_context(None)
            .map_err(|e| anyhow!("Could not clear machine context: {e}"))?;
        println!("\u{2713} Machine context cleared (this install now counts as human).");
        return Ok(());
    }
    let Some(value) = value else {
        match telemetry::machine_context() {
            Some(c) => println!("Machine context: {c}"),
            None => println!("Machine context: (none — this install counts as human)"),
        }
        return Ok(());
    };
    let value = value.trim();
    if value.is_empty() || value.len() > 64 || value.chars().any(|c| c.is_whitespace()) {
        return Err(anyhow!(
            "context must be a single label of at most 64 characters (e.g. cloud-agent)"
        ));
    }
    telemetry::set_machine_context(Some(value.to_string()))
        .map_err(|e| anyhow!("Could not save machine context: {e}"))?;
    println!(
        "\u{2713} Machine context set to \"{value}\" (events are tagged install_kind={value})."
    );
    Ok(())
}

fn status() -> Result<()> {
    // `--no-telemetry` is a per-run flag, not persisted state, so status reports
    // the standing (persisted) setting with flag=false.
    match telemetry::disabled_reason(false) {
        None => {
            println!("Anonymous usage analytics: on");
            println!("  No code, prompts, file contents, or identifiers are ever sent.");
        }
        Some(reason) => {
            println!("Anonymous usage analytics: off ({})", reason.as_str());
            // The fork default deserves a pointer, not just a reason string:
            // there's nothing wrong to fix, there's simply no destination yet.
            if matches!(reason, telemetry::DisabledReason::NoKey) {
                println!("  Crux ships no PostHog key, so nothing is sent anywhere. Point it at");
                println!("  your own project with `orx telemetry key phc_...` if you want it.");
            }
        }
    }

    match telemetry::load_settings().and_then(|s| s.install_id) {
        Some(id) => println!("  Anonymous install id: {id}"),
        None => println!("  Anonymous install id: (not yet generated)"),
    }
    if let Some(context) = telemetry::machine_context() {
        println!("  Machine context: {context} (events tagged install_kind={context})");
    }
    println!();
    println!("Turn off with `orx telemetry off`; back on with `orx telemetry on`.");
    Ok(())
}

async fn set_enabled(enabled: bool) -> Result<()> {
    // Record the consent decision itself — unconditionally, so an opt-out is
    // still counted (see telemetry::record_consent).
    telemetry::record_consent(enabled).await;
    telemetry::set_persisted_disabled(!enabled)
        .map_err(|e| anyhow!("Could not save telemetry setting: {e}"))?;
    if enabled {
        println!("\u{2713} Anonymous usage analytics enabled.");
        println!("  (The --no-telemetry flag still disables it for a single run.)");
    } else {
        println!("\u{2713} Anonymous usage analytics disabled on this machine.");
    }
    Ok(())
}
