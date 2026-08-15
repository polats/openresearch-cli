//! Inspect a directory on this machine so a project can be created from a repo
//! the user already has checked out.
//!
//! Read-only, and deliberately shallow: it reports the *shape* of what is at a
//! path — is it a repo, does it have an origin, does that origin point at GitHub
//! — and never file contents. The dashboard is unauthenticated by design, so a
//! route that reaches outside the data dir earns its keep only by answering the
//! narrowest possible question.
//!
//! What it does **not** do is change how a project works. A resolved
//! `owner/repo` goes through the same create path the "Existing repo" tab uses,
//! so the chosen directory is only ever read; crux still clones into its own
//! cache. Everything downstream — `ensure_clone` and its eleven callers, the run
//! backends, session worktrees — is untouched, which is the point.

use std::path::Path;

use serde::Serialize;

/// What is at the path, in the four shapes the form has to explain. Each one
/// has a different fix, so they are distinguished here rather than collapsed
/// into a bool the UI would have to guess at.
// `rename_all` covers the variant names (the `kind` tag); `rename_all_fields`
// covers their fields — without the second, `current_branch` ships snake_case
// while every other payload the UI reads is camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum LocalRepo {
    /// Not a directory, or not readable.
    Missing,
    /// A directory, but not a git checkout.
    NotARepo,
    /// A git checkout with no `origin`. Publishable.
    NoRemote {
        /// Branch to publish, when there is one. `None` on a detached HEAD or a
        /// repo with no commits yet — neither can be pushed as-is.
        current_branch: Option<String>,
        /// Suggested name for the repo that would be created.
        suggested_name: Option<String>,
    },
    /// A git checkout whose `origin` is not GitHub. Reported rather than
    /// accepted: the run backends clone `github.com` literally, so this would
    /// fail at the first run rather than here.
    ForeignRemote { remote_url: String },
    /// A git checkout with a GitHub `origin` — the case that just works.
    #[serde(rename = "github")]
    Github {
        owner: String,
        repo: String,
        remote_url: String,
        current_branch: Option<String>,
    },
}

/// Inspect `path`.
pub fn inspect(path: &str) -> LocalRepo {
    let expanded = expand_tilde(path.trim());
    let path = Path::new(&expanded);
    let Some(root) = crate::local::git::repo_root(path) else {
        // Distinguish "you typed a path that isn't there" from "that's a folder
        // but not a repo" — the fixes are completely different.
        return if path.is_dir() {
            LocalRepo::NotARepo
        } else {
            LocalRepo::Missing
        };
    };

    let current_branch = crate::local::git::current_branch(&root);
    let Some(remote_url) = crate::local::git::remote_url(&root, "origin") else {
        return LocalRepo::NoRemote {
            current_branch,
            suggested_name: root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty()),
        };
    };

    match parse_github_remote(&remote_url) {
        Some((owner, repo)) => LocalRepo::Github {
            owner,
            repo,
            remote_url,
            current_branch,
        },
        None => LocalRepo::ForeignRemote { remote_url },
    }
}

/// `~` and `~/…` against the home dir. The field is typed by hand, and a path
/// starting with `~` is what people actually write.
pub fn expand_tilde(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        // `~user/…` — not ours to resolve; leave it for the OS to reject.
        return path.to_string();
    }
    match dirs::home_dir() {
        Some(home) => format!("{}{}", home.display(), rest),
        None => path.to_string(),
    }
}

/// `owner/repo` from a GitHub remote URL, in the forms git actually writes.
///
/// Both spellings matter and neither is exotic: `git clone` over SSH produces the
/// `git@` scp-like form, over HTTPS the URL form, and a repo can carry either.
/// Anything else — GitLab, a bare path, a git:// URL — returns `None` so the
/// caller can say *why* rather than failing later inside a run.
fn parse_github_remote(url: &str) -> Option<(String, String)> {
    /// Every spelling of a github.com remote git writes. Anything not on this
    /// list — a URL carrying credentials, a subdomain-style host — is
    /// deliberately unmatched: guessing risks resolving to the wrong account.
    const PREFIXES: [&str; 5] = [
        // scp-like, what an SSH clone writes
        "git@github.com:",
        "ssh://git@github.com/",
        "https://github.com/",
        "http://github.com/",
        "git://github.com/",
    ];
    let url = url.trim();
    let rest = PREFIXES.iter().find_map(|p| url.strip_prefix(p))?;

    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, repo) = rest.split_once('/')?;
    // Exactly two segments. `owner/repo/extra` is not a repo URL, and accepting
    // it would produce a plausible-looking pair that clones to nothing.
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_two_forms_git_actually_writes() {
        // HTTPS — what `~/projects/<game>` carries.
        assert_eq!(
            parse_github_remote("https://github.com/acme/example-game.git"),
            Some(("acme".into(), "example-game".into()))
        );
        // scp-like SSH — what an SSH clone carries.
        assert_eq!(
            parse_github_remote("git@github.com:trinity-asylum/crux.git"),
            Some(("trinity-asylum".into(), "crux".into()))
        );
        // The `.git` suffix is optional in both, and a trailing slash happens.
        assert_eq!(
            parse_github_remote("https://github.com/o/r"),
            Some(("o".into(), "r".into()))
        );
        assert_eq!(
            parse_github_remote("https://github.com/o/r/"),
            Some(("o".into(), "r".into()))
        );
        assert_eq!(
            parse_github_remote("ssh://git@github.com/o/r.git"),
            Some(("o".into(), "r".into()))
        );
    }

    /// Non-GitHub remotes must return `None` so the caller reports them as
    /// unsupported *here*. Accepting one would produce an owner/repo that only
    /// fails much later, inside a run, where the message makes no sense.
    #[test]
    fn rejects_remotes_that_are_not_github() {
        for url in [
            "https://gitlab.com/o/r.git",
            "git@gitlab.com:o/r.git",
            "https://bitbucket.org/o/r.git",
            "/srv/git/bare.git",
            "file:///srv/git/bare.git",
            "https://github.example.com/o/r.git",
            "https://notgithub.com/o/r.git",
        ] {
            assert_eq!(parse_github_remote(url), None, "wrongly accepted: {url}");
        }
    }

    /// A repo URL has exactly two segments. Anything else would resolve to a
    /// pair that looks right and clones to nothing.
    #[test]
    fn rejects_malformed_github_urls() {
        for url in [
            "https://github.com/",
            "https://github.com/owner",
            "https://github.com/owner/",
            "https://github.com//repo",
            "https://github.com/owner/repo/tree/main",
            "",
        ] {
            assert_eq!(parse_github_remote(url), None, "wrongly accepted: {url}");
        }
    }

    #[test]
    fn expands_tilde_paths() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~"), home.display().to_string());
        assert_eq!(
            expand_tilde("~/projects/x"),
            format!("{}/projects/x", home.display())
        );
        // Absolute and relative paths pass through untouched…
        assert_eq!(expand_tilde("/tmp/x"), "/tmp/x");
        assert_eq!(expand_tilde("./x"), "./x");
        // …as does `~user`, which is not ours to resolve.
        assert_eq!(expand_tilde("~other/x"), "~other/x");
    }

    /// A path that doesn't exist and a directory that isn't a repo are different
    /// problems with different fixes, so they must not collapse into one state.
    #[test]
    fn distinguishes_missing_from_not_a_repo() {
        assert_eq!(
            inspect("/definitely/not/a/real/path/orx-test"),
            LocalRepo::Missing
        );
        let dir = std::env::temp_dir().join(format!("orx-notarepo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(inspect(&dir.display().to_string()), LocalRepo::NotARepo);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file is not a directory, and must not read as a repo.
    #[test]
    fn a_file_is_missing_not_a_repo() {
        let f = std::env::temp_dir().join(format!("orx-file-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&f, "x").unwrap();
        assert_eq!(inspect(&f.display().to_string()), LocalRepo::Missing);
        let _ = std::fs::remove_file(&f);
    }
}
