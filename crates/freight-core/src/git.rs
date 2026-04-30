//! Git dependency operations backed by libgit2 (the `git2` crate).
//!
//! Using libgit2 keeps freight self-contained — no `git` binary required on
//! `$PATH` — and gives us progress callbacks, SSH-agent auth, and credential
//! helpers without any subprocess wrangling.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use git2::{
    build::RepoBuilder, Cred, CredentialType, FetchOptions, RemoteCallbacks, Repository,
    ResetType,
};

use crate::error::FreightError;

// ── Public API ────────────────────────────────────────────────────────────────

/// Clone `url` into `dest`. The directory must not already exist.
///
/// Ref resolution order:
/// 1. `rev` — full clone then `git checkout <sha>` (shallow can't guarantee
///    the SHA is reachable).
/// 2. `branch` or `tag` — clone the named branch/tag.
/// 3. Neither — clone the remote's default branch.
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
    } else {
        // --branch works for both branch and tag names in git2.
        let ref_name = branch.or(tag);
        with_auth_progress(|mut fo| {
            attach_progress(&mut fo, Arc::clone(&last_pct));
            let mut builder = RepoBuilder::new();
            if let Some(r) = ref_name {
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
    let _ = {
        let mut remote = repo.find_remote("origin").ok();
        if let Some(ref mut r) = remote {
            with_auth(|fo| r.fetch(&["refs/heads/*:refs/remotes/origin/*"], Some(&mut { fo }), None))
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
