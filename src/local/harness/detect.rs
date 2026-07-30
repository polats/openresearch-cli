//! Shared detection primitives for the harness registry — the wire types every
//! harness reports (`HarnessInfo`, `ModelInfo`) and the best-effort probes
//! (`--version`, auth-file reads, JWT decode) the per-harness impls build on.
//!
//! Detection is read-only and best-effort: missing files or unparseable JSON
//! just mean "not detected", never an error.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

pub(super) const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    /// Reasoning/effort choices this *specific* model accepts, led by the
    /// `Default` sentinel. `None` means "this model has no list of its own" —
    /// the composer then falls back to the harness-wide
    /// [`HarnessOptions::reasoning_levels`](super::HarnessOptions).
    ///
    /// `Some(vec![])` is meaningfully different from `None`: it means the model
    /// was *checked* and genuinely exposes no reasoning control (an OpenCode
    /// model with an empty `variants` map), so the picker is hidden entirely
    /// rather than falling back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_levels: Option<Vec<super::options::OptionChoice>>,
    /// The catalog's own human name for the model (`Opus`, `GPT-5.6 Sol`,
    /// `Big Pickle`). Absent for statically-listed fallback models, where the
    /// UI derives a label from the id instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The catalog's one-line blurb — for Claude this is where the resolved
    /// version lives (`Opus 4.8 with 1M context · Best for everyday, complex
    /// tasks`), since its picker aliases (`opus[1m]`) are unversioned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The tier that actually runs when the user picks nothing — set only when
    /// the CLI reports it (codex's `defaultReasoningEffort`, resolved against a
    /// `config.toml` override). When present, `reasoning_levels` carries no
    /// `default` sentinel and the composer preselects this concrete tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_level: Option<String>,
}

impl ModelInfo {
    /// A model with no per-model reasoning metadata (falls back to the
    /// harness-wide list).
    pub(super) fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            reasoning_levels: None,
            display_name: None,
            description: None,
            default_reasoning_level: None,
        }
    }

    /// Attach reasoning choices *with a known concrete default*: no sentinel
    /// row, the default tier preselected instead. For catalogs that report
    /// which tier runs when nothing is chosen (codex).
    pub(super) fn with_reasoning_default(mut self, ids: &[&str], default: &str) -> Self {
        self.reasoning_levels = Some(super::options::reasoning_tiers(ids));
        // A default outside the advertised tiers would be unselectable — leave
        // it unset then, and the composer preselects the first tier.
        self.default_reasoning_level = ids.contains(&default).then(|| default.to_string());
        self
    }

    /// Attach the catalog's display name / description, when it has them.
    pub(super) fn with_label(
        mut self,
        display_name: Option<&str>,
        description: Option<&str>,
    ) -> Self {
        self.display_name = display_name.map(str::to_string);
        self.description = description.map(str::to_string);
        self
    }

    /// Attach this model's own reasoning choices, from native ids. An empty
    /// `ids` yields an empty (not absent) list — "checked, none supported".
    pub(super) fn with_reasoning(mut self, ids: &[&str]) -> Self {
        self.reasoning_levels = Some(if ids.is_empty() {
            Vec::new()
        } else {
            super::options::reasoning_choices(ids)
        });
        self
    }
}

/// One quota bucket of a harness's plan (a rolling window like "5h" or
/// "Weekly"), normalized across providers to **percent remaining** so the UI
/// renders every harness the same way. Claude reports `utilization` (percent
/// used); Codex reports `used_percent`; both become `100 - used` here.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    /// Short human label, e.g. `"5h"`, `"Weekly"`, `"Weekly (Sonnet)"`.
    pub label: String,
    /// Percent of the window still available, 0–100.
    pub remaining_percent: f64,
    /// When the window resets (epoch ms), if the provider tells us.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<i64>,
}

/// Remaining-usage summary for one harness, shown in the Settings → Harnesses
/// tab. `windows` carries the plan quota buckets (Claude/Codex); `note` +
/// `manage_url` cover harnesses with no usage API (OpenCode Zen — balance is
/// dashboard-only). Any subset may be empty; the UI renders whatever is present.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessUsage {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<UsageWindow>,
    /// When this snapshot was observed (epoch ms) — drives the "as of" line for
    /// captured (not live-fetched) data like Codex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<i64>,
    /// A short explanation shown when there are no windows (e.g. Zen).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// External link to manage/top-up usage (Zen dashboard).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manage_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A signed-in setup was found (auth file / OAuth account).
    pub authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<&'static str>, // "oauth" | "apiKey"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Usable as a chat backend right now (installed + signed in).
    pub agent_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_note: Option<String>,
    pub models: Vec<ModelInfo>,
    /// Composer toggle vocabulary (permission modes, reasoning levels).
    pub options: super::HarnessOptions,
    /// Remaining plan quota, when the harness exposes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<HarnessUsage>,
}

impl HarnessInfo {
    pub(super) fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            installed: false,
            bin_path: None,
            version: None,
            authenticated: false,
            auth_method: None,
            account: None,
            org: None,
            plan: None,
            agent_ready: false,
            agent_note: None,
            models: Vec::new(),
            options: super::HarnessOptions::none(),
            usage: None,
        }
    }

    /// Attach the chat model list. Each `ModelInfo` carries its own reasoning
    /// choices where the harness knows them (issue #123).
    pub(super) fn with_models(mut self, models: Vec<ModelInfo>) -> Self {
        self.models = models;
        self
    }
}

pub(super) fn find_on_path(bin: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(bin))
        .find(|c| c.is_file())
}

/// Dereference symlinks to the real installed binary. Installers commonly drop
/// a lone symlink into `~/.local/bin`, but some CLIs locate sibling helper
/// executables relative to the path they were *invoked as*, without resolving
/// symlinks — codex >= 0.144 launches `codex-code-mode-host` this way and every
/// command fails with "No such file or directory" when codex is spawned via the
/// symlink. Spawning the resolved path keeps helpers real siblings. Best-effort:
/// a path that can't be resolved is returned unchanged.
pub(super) fn resolve_symlinks(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

/// `<bin> --version`, first line, with a timeout (node CLIs can be slow).
pub(super) async fn bin_version(bin: &PathBuf) -> Option<String> {
    let fut = tokio::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output();
    let out = tokio::time::timeout(VERSION_TIMEOUT, fut)
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    (!line.is_empty()).then_some(line)
}

/// An API key from the process env, else orx's own synced env file — the two
/// sources `prepare_env` actually hands the harness child. Detecting only the
/// former would report a working setup as signed out.
pub(super) fn api_key(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| crate::config::synced_env_var(key))
}

pub(super) fn read_json(path: PathBuf) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(super) fn nonempty_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Decode a JWT's payload without verifying — we only surface the account
/// email and plan the user is already signed in as, locally.
pub(super) fn jwt_payload(token: &str) -> Option<Value> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Best-effort authenticated GET returning the JSON body. Short timeout, no
/// retries — a usage probe must never slow detection or fail it: any error
/// (offline, 401, timeout, non-JSON) just yields `None` and the harness renders
/// without a usage row. `headers` are `(name, value)` pairs.
pub(super) async fn get_json_authed(url: &str, headers: &[(&str, String)]) -> Option<Value> {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default()
    });
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<Value>().await.ok()
}

/// Parse an RFC-3339 / ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SS[.fff][Z]`)
/// to epoch milliseconds, so a provider's ISO reset time normalizes to the same
/// numeric `resetsAtMs` Codex's unix-seconds resets produce. Best-effort:
/// fractional seconds and a trailing `Z` are ignored, a numeric `+HH:MM` offset
/// is applied, and anything unparseable returns `None`. Not a general date
/// library — just enough for the usage reset fields.
pub(super) fn iso8601_to_epoch_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T').or_else(|| s.split_once(' '))?;
    let mut dp = date.split('-');
    let year: i64 = dp.next()?.parse().ok()?;
    let month: i64 = dp.next()?.parse().ok()?;
    let day: i64 = dp.next()?.parse().ok()?;

    // Strip the zone suffix off the time, capturing any numeric offset.
    let mut offset_min: i64 = 0;
    let mut time = rest;
    if let Some(t) = time.strip_suffix('Z').or_else(|| time.strip_suffix('z')) {
        time = t;
    } else if let Some(idx) = time.rfind(['+', '-']) {
        // Only treat +/- as an offset if it follows the time (not the very
        // first char) and looks like HH:MM / HHMM.
        if idx > 0 {
            let sign = if &time[idx..idx + 1] == "-" { -1 } else { 1 };
            let off = &time[idx + 1..];
            let (oh, om) = match off.split_once(':') {
                Some((h, m)) => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?),
                None if off.len() == 4 => {
                    (off[..2].parse::<i64>().ok()?, off[2..].parse::<i64>().ok()?)
                }
                None => (off.parse::<i64>().ok()?, 0),
            };
            offset_min = sign * (oh * 60 + om);
            time = &time[..idx];
        }
    }
    let time = time.split('.').next().unwrap_or(time); // drop fractional seconds
    let mut tp = time.split(':');
    let hour: i64 = tp.next()?.parse().ok()?;
    let minute: i64 = tp.next().unwrap_or("0").parse().ok()?;
    let second: i64 = tp.next().unwrap_or("0").parse().ok()?;

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_min * 60;
    Some(secs * 1_000)
}

/// Days since the Unix epoch for a proleptic-Gregorian date (Howard Hinnant's
/// `days_from_civil`). Valid for any date; used only by `iso8601_to_epoch_ms`.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse a `major.minor.patch` triple out of a `--version` line. The first
/// whitespace-separated token that parses wins, so `"codex-cli 0.144.0"`,
/// `"2.1.197 (Claude Code)"`, and a bare `"0.144.0"` all resolve; a `-suffix`
/// on the patch is tolerated. `None` when no token has the shape, which each
/// caller treats as "assume the older behaviour".
pub(super) fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    version.split_whitespace().find_map(|token| {
        let mut parts = token.splitn(3, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts
            .next()?
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?;
        Some((major, minor, patch))
    })
}

pub(super) fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn resolve_symlinks_dereferences_to_real_binary() {
        let dir = std::env::temp_dir().join(format!("orx-detect-test-{}", std::process::id()));
        let install = dir.join("install");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let real = install.join("codex");
        std::fs::write(&real, "").unwrap();
        let link = bin.join("codex");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert_eq!(resolve_symlinks(link), real.canonicalize().unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn iso8601_parses_utc_and_offsets() {
        // Epoch.
        assert_eq!(iso8601_to_epoch_ms("1970-01-01T00:00:00Z"), Some(0));
        // A known instant: 2025-07-23T18:00:00Z == 1753293600 s.
        assert_eq!(
            iso8601_to_epoch_ms("2025-07-23T18:00:00Z"),
            Some(1_753_293_600_000)
        );
        // Fractional seconds are dropped, not fatal.
        assert_eq!(
            iso8601_to_epoch_ms("2025-07-23T18:00:00.123Z"),
            Some(1_753_293_600_000)
        );
        // A +02:00 offset shifts back to UTC (same wall time, 2h earlier epoch).
        assert_eq!(
            iso8601_to_epoch_ms("2025-07-23T20:00:00+02:00"),
            Some(1_753_293_600_000)
        );
        // Junk is None, never a panic.
        assert_eq!(iso8601_to_epoch_ms("not-a-date"), None);
        assert_eq!(iso8601_to_epoch_ms(""), None);
    }

    #[test]
    fn resolve_symlinks_keeps_unresolvable_path() {
        let missing = PathBuf::from("/nonexistent/orx-detect-test/codex");
        assert_eq!(resolve_symlinks(missing.clone()), missing);
    }
}
