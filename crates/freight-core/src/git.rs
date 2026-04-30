//! Git dependency operations backed by libgit2 (the `git2` crate).
//!
//! Using libgit2 keeps freight self-contained — no `git` binary required on
//! `$PATH` — and gives us progress callbacks, SSH-agent auth, and credential
//! helpers without any subprocess wrangling.

use std::path::Path;

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
    if let Some(sha) = rev {
        // Full clone so the arbitrary commit is reachable, then detach to it.
        let repo = with_auth(|fo| {
            RepoBuilder::new().fetch_options(fo).clone(url, dest)
        })?;
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
        with_auth(|fo| {
            let mut builder = RepoBuilder::new();
            if let Some(r) = ref_name {
                builder.branch(r);
            }
            builder.fetch_options(fo).clone(url, dest)
        })?;
    }
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

    with_auth(|mut fo| {
        remote.fetch(&[&remote_ref], Some(&mut fo), None)
    })?;

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

// ── Auth helper ───────────────────────────────────────────────────────────────

/// Run a git2 operation that needs a `FetchOptions`, supplying credential
/// callbacks that try SSH agent first, then the system credential helper.
fn with_auth<T, F>(f: F) -> Result<T, FreightError>
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
