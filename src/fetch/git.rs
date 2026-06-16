//! Git dependency operations backed by libgit2 (the `git2` crate).
//!
//! Using libgit2 keeps freight self-contained — no `git` binary required on
//! `$PATH` — and gives us progress callbacks, SSH-agent auth, and credential
//! helpers without any subprocess wrangling.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use git2::{
    build::RepoBuilder, Cred, CredentialType, FetchOptions, RemoteCallbacks, Repository, ResetType,
};

use crate::error::FreightError;

// ── Public API ────────────────────────────────────────────────────────────────

/// Clone `url` into `dest`. The directory must not already exist.
///
/// Ref resolution order:
/// 1. `rev` — full clone then detach to `<sha>` (shallow can't guarantee the
///    SHA is reachable).
/// 2. `tag` — clone the default branch, then detach to the tag (libgit2's
///    branch-clone path doesn't resolve tags).
/// 3. `branch` — clone that branch directly.
/// 4. None — clone the remote's default branch.
pub fn clone_dep(
    dest: &Path,
    url: &str,
    branch: Option<&str>,
    tag: Option<&str>,
    rev: Option<&str>,
) -> Result<(), FreightError> {
    let last_pct = Arc::new(AtomicUsize::new(0));

    if let Some(sha) = rev {
        // Full clone so the arbitrary commit is reachable, then detach to it.
        let repo = with_auth_progress(|mut fo| {
            attach_progress(&mut fo, Arc::clone(&last_pct));
            RepoBuilder::new().fetch_options(fo).clone(url, dest)
        })?;
        eprint!("\r");
        let obj = repo
            .revparse_single(sha)
            .map_err(|e| FreightError::GitError(format!("rev `{sha}` not found: {e}")))?;
        repo.checkout_tree(&obj, None)
            .map_err(|e| FreightError::GitError(format!("checkout `{sha}`: {e}")))?;
        repo.set_head_detached(obj.id())
            .map_err(|e| FreightError::GitError(e.to_string()))?;
    } else if let Some(tag) = tag {
        // libgit2's `RepoBuilder::branch()` only resolves remote-tracking
        // branches (`refs/remotes/origin/<name>`), not tags — so clone the
        // default branch, then check out the tag in detached HEAD ourselves.
        let repo = with_auth_progress(|mut fo| {
            attach_progress(&mut fo, Arc::clone(&last_pct));
            RepoBuilder::new().fetch_options(fo).clone(url, dest)
        })?;
        eprint!("\r");
        let obj = repo
            .revparse_single(&format!("refs/tags/{tag}"))
            .or_else(|_| repo.revparse_single(tag))
            .map_err(|e| FreightError::GitError(format!("tag `{tag}` not found: {e}")))?;
        // Peel handles both annotated and lightweight tags.
        let commit = obj
            .peel_to_commit()
            .map_err(|e| FreightError::GitError(format!("tag `{tag}`: {e}")))?;
        repo.checkout_tree(commit.as_object(), None)
            .map_err(|e| FreightError::GitError(format!("checkout tag `{tag}`: {e}")))?;
        repo.set_head_detached(commit.id())
            .map_err(|e| FreightError::GitError(e.to_string()))?;
    } else {
        // Branch (or the remote's default branch when None).
        with_auth_progress(|mut fo| {
            attach_progress(&mut fo, Arc::clone(&last_pct));
            let mut builder = RepoBuilder::new();
            if let Some(r) = branch {
                builder.branch(r);
            }
            builder.fetch_options(fo).clone(url, dest)
        })?;
        eprint!("\r");
    }
    Ok(())
}

/// Checkout a specific commit SHA in an already-cloned repo.
///
/// Used to enforce a locked or pinned rev without re-cloning.
pub fn checkout_rev(dest: &Path, sha: &str) -> Result<(), FreightError> {
    let repo = Repository::open(dest)
        .map_err(|e| FreightError::GitError(format!("open repo at {}: {e}", dest.display())))?;

    // Fetch so the SHA is reachable even if the clone was shallow or branched.
    {
        let mut remote = repo.find_remote("origin").ok();
        if let Some(ref mut r) = remote {
            with_auth(|fo| {
                r.fetch(
                    &["refs/heads/*:refs/remotes/origin/*"],
                    Some(&mut { fo }),
                    None,
                )
            })
            .ok(); // non-fatal — SHA may already be local
        }
    };

    let obj = repo
        .revparse_single(sha)
        .map_err(|e| FreightError::GitError(format!("rev `{sha}` not found: {e}")))?;
    repo.checkout_tree(&obj, None)
        .map_err(|e| FreightError::GitError(format!("checkout `{sha}`: {e}")))?;
    repo.set_head_detached(obj.id())
        .map_err(|e| FreightError::GitError(e.to_string()))?;
    Ok(())
}

/// Pull the latest commits into an already-cloned dep directory.
///
/// - `rev`-pinned deps are skipped (returns `Ok(())` immediately).
/// - Otherwise fetches origin and hard-resets the working tree.
pub fn update_dep(
    dest: &Path,
    branch: Option<&str>,
    tag: Option<&str>,
    rev: Option<&str>,
) -> Result<(), FreightError> {
    if rev.is_some() {
        return Ok(());
    }

    let repo = Repository::open(dest)
        .map_err(|e| FreightError::GitError(format!("open repo at {}: {e}", dest.display())))?;

    // Determine what to fetch and reset to.
    let remote_ref = if let Some(b) = branch.or(tag) {
        format!("refs/heads/{b}")
    } else {
        // Follow HEAD's upstream branch.
        let head = repo
            .head()
            .map_err(|e| FreightError::GitError(format!("HEAD: {e}")))?;
        head.shorthand()
            .map(|s| format!("refs/heads/{s}"))
            .unwrap_or_else(|| "HEAD".to_string())
    };

    let mut remote = repo
        .find_remote("origin")
        .map_err(|e| FreightError::GitError(format!("remote origin: {e}")))?;

    let last_pct = Arc::new(AtomicUsize::new(0));
    with_auth_progress(|mut fo| {
        attach_progress(&mut fo, Arc::clone(&last_pct));
        remote.fetch(&[&remote_ref], Some(&mut fo), None)
    })?;
    eprint!("\r");

    // Reset hard to FETCH_HEAD.
    let fetch_head = repo
        .find_reference("FETCH_HEAD")
        .map_err(|e| FreightError::GitError(format!("FETCH_HEAD: {e}")))?;
    let obj = fetch_head
        .peel(git2::ObjectType::Commit)
        .map_err(|e| FreightError::GitError(e.to_string()))?;
    repo.reset(&obj, ResetType::Hard, None)
        .map_err(|e| FreightError::GitError(format!("reset: {e}")))?;

    Ok(())
}

/// Return the full commit SHA currently checked out in `dest`, or `None` when
/// the path is not a git repo.
pub fn current_rev(dest: &Path) -> Option<String> {
    let repo = Repository::open(dest).ok()?;
    let head = repo.head().ok()?;
    let commit = head.peel_to_commit().ok()?;
    Some(commit.id().to_string())
}

// ── Auth helpers ──────────────────────────────────────────────────────────────

/// Run a git2 operation that needs a `FetchOptions` (no progress).
fn with_auth<T, F>(f: F) -> Result<T, FreightError>
where
    F: FnOnce(FetchOptions<'_>) -> Result<T, git2::Error>,
{
    with_auth_progress(f)
}

/// Run a git2 operation that needs a `FetchOptions`, wiring up credentials.
/// The caller is responsible for attaching any progress callbacks before use.
fn with_auth_progress<T, F>(f: F) -> Result<T, FreightError>
where
    F: FnOnce(FetchOptions<'_>) -> Result<T, git2::Error>,
{
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|_url, username, allowed| {
        if allowed.contains(CredentialType::SSH_KEY) {
            let user = username.unwrap_or("git");
            return Cred::ssh_key_from_agent(user);
        }
        if allowed.contains(CredentialType::DEFAULT) {
            return Cred::default();
        }
        Err(git2::Error::from_str("no suitable credentials available"))
    });

    let mut fo = FetchOptions::new();
    fo.remote_callbacks(callbacks);
    f(fo).map_err(|e| FreightError::GitError(e.to_string()))
}

/// Attach a compact transfer-progress callback to `fo` that prints a single
/// updating line like `  receiving objects  42%  (1234/2912)`.
fn attach_progress(fo: &mut FetchOptions<'_>, last_pct: Arc<AtomicUsize>) {
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|_url, username, allowed| {
        if allowed.contains(CredentialType::SSH_KEY) {
            return Cred::ssh_key_from_agent(username.unwrap_or("git"));
        }
        if allowed.contains(CredentialType::DEFAULT) {
            return Cred::default();
        }
        Err(git2::Error::from_str("no suitable credentials available"))
    });
    callbacks.transfer_progress(move |stats| {
        if stats.total_objects() == 0 {
            return true;
        }
        let pct = stats.received_objects() * 100 / stats.total_objects();
        let prev = last_pct.swap(pct, Ordering::Relaxed);
        if pct != prev || pct == 100 {
            eprint!(
                "\r    receiving objects {:>3}%  ({}/{})",
                pct,
                stats.received_objects(),
                stats.total_objects(),
            );
        }
        true
    });
    fo.remote_callbacks(callbacks);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Set up a tiny local git repo with one commit tagged `v1.0`. Returns the
    /// repo path, or `None` when the `git` CLI isn't available (test skips).
    fn local_repo_with_tag() -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().ok()?;
        let p = dir.path();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(p)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
        };
        if !git(&["init", "-q"]).ok()?.status.success() {
            return None;
        }
        std::fs::write(p.join("marker.txt"), "hi").ok()?;
        git(&["add", "."]).ok()?;
        git(&["commit", "-qm", "init"]).ok()?;
        git(&["tag", "v1.0"]).ok()?;
        Some(dir)
    }

    fn git_in(p: &Path, args: &[&str]) -> Option<std::process::Output> {
        Command::new("git")
            .args(args)
            .current_dir(p)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .ok()
    }

    fn file_url(p: &Path) -> String {
        format!("file://{}", p.display())
    }

    #[test]
    fn clone_dep_checks_out_a_tag() {
        // Regression: a `tag` used to be passed to RepoBuilder::branch(), which
        // resolves only remote-tracking branches → "refs/remotes/origin/<tag>
        // not found". A tag must be checked out in detached HEAD instead.
        let Some(origin) = local_repo_with_tag() else {
            eprintln!("skipping: git CLI unavailable");
            return;
        };
        let work = tempfile::tempdir().unwrap();
        let dest = work.path().join("clone");
        clone_dep(&dest, &file_url(origin.path()), None, Some("v1.0"), None).expect("clone by tag");
        assert!(
            dest.join("marker.txt").exists(),
            "tagged content should be checked out"
        );
    }

    #[test]
    fn clone_dep_checks_out_a_branch() {
        let origin = tempfile::tempdir().unwrap();
        let p = origin.path();
        if git_in(p, &["init", "-q"]).map(|o| o.status.success()) != Some(true) {
            eprintln!("skipping: git CLI unavailable");
            return;
        }
        std::fs::write(p.join("base.txt"), "base").unwrap();
        git_in(p, &["add", "."]);
        git_in(p, &["commit", "-qm", "base"]);
        let default = String::from_utf8(git_in(p, &["branch", "--show-current"]).unwrap().stdout)
            .unwrap()
            .trim()
            .to_string();
        git_in(p, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(p.join("feature.txt"), "feat").unwrap();
        git_in(p, &["add", "."]);
        git_in(p, &["commit", "-qm", "feature"]);
        git_in(p, &["checkout", "-q", &default]); // default branch lacks feature.txt

        let work = tempfile::tempdir().unwrap();
        let dest = work.path().join("clone");
        clone_dep(&dest, &file_url(p), Some("feature"), None, None).expect("clone by branch");
        assert!(
            dest.join("feature.txt").exists(),
            "the feature branch should be checked out"
        );
    }

    #[test]
    fn clone_dep_checks_out_a_rev() {
        let origin = tempfile::tempdir().unwrap();
        let p = origin.path();
        if git_in(p, &["init", "-q"]).map(|o| o.status.success()) != Some(true) {
            eprintln!("skipping: git CLI unavailable");
            return;
        }
        std::fs::write(p.join("first.txt"), "1").unwrap();
        git_in(p, &["add", "."]);
        git_in(p, &["commit", "-qm", "first"]);
        let rev = String::from_utf8(git_in(p, &["rev-parse", "HEAD"]).unwrap().stdout)
            .unwrap()
            .trim()
            .to_string();
        std::fs::write(p.join("second.txt"), "2").unwrap();
        git_in(p, &["add", "."]);
        git_in(p, &["commit", "-qm", "second"]);

        let work = tempfile::tempdir().unwrap();
        let dest = work.path().join("clone");
        clone_dep(&dest, &file_url(p), None, None, Some(&rev)).expect("clone by rev");
        assert!(dest.join("first.txt").exists(), "rev content present");
        assert!(
            !dest.join("second.txt").exists(),
            "later commit must not be present at the pinned rev"
        );
    }
}
