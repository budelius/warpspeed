//! Talking to a code forge — GitHub, GitLab or Gitea — over its HTTP API,
//! rather than by shelling out to a per-forge CLI.
//!
//! The forge is a property of a *remote*, not of a repository: a repository can
//! have remotes on two different forges, and this one does.
//!
//! Landing in stages, per `docs/superpowers/specs/2026-09-08-forge-layer-design.md`.
//! Stage one is this file: the types and remote-URL parsing, which are pure and
//! fully tested but have no caller yet — the first one arrives with the GitHub
//! client in stage four, when `gh pr view` is replaced. Drop the `dead_code`
//! allowance then.
#![allow(dead_code)]

pub mod credentials;
pub mod os_credentials;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

/// Which forge software a host runs.
///
/// A self-hosted GitLab or Gitea cannot be told apart by hostname, so anything
/// outside the public hosts has to be probed. See `detect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ForgeKind {
    GitHub,
    GitLab,
    Gitea,
}

impl ForgeKind {
    /// The forge behind a host that needs no probing.
    ///
    /// Only the public hosts qualify. GitHub Enterprise, self-hosted GitLab and
    /// Gitea all live on arbitrary names and must be discovered.
    pub fn for_known_host(host: &str) -> Option<Self> {
        match host {
            "github.com" | "www.github.com" => Some(Self::GitHub),
            "gitlab.com" | "www.gitlab.com" => Some(Self::GitLab),
            _ => None,
        }
    }
}

/// A repository on a forge, parsed from a git remote URL.
///
/// `owner` is the whole namespace path rather than one segment, because GitLab
/// nests projects under groups and subgroups; addressing `group/subgroup/project`
/// as owner `group` would point at a different project or none at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub host: String,
    pub port: Option<u16>,
    pub owner: String,
    pub name: String,
}

impl RepoRef {
    /// Parses a git remote URL in either the URL form
    /// (`https://host/owner/name.git`, `ssh://git@host:2222/owner/name.git`) or
    /// the scp-like form (`git@host:owner/name.git`). Both appear in this
    /// repository's own remotes.
    ///
    /// Returns `None` for anything that does not name a project.
    pub fn from_remote_url(url: &str) -> Option<Self> {
        let url = url.trim();
        if url.is_empty() {
            return None;
        }

        let (host_port, path) = if let Some((_scheme, rest)) = url.split_once("://") {
            let rest = strip_userinfo(rest);
            let (host_port, path) = rest.split_once('/')?;
            (host_port, path)
        } else {
            // scp-like: `[user@]host:path`, which has no port field at all.
            let rest = strip_userinfo(url);
            let (host, path) = rest.split_once(':')?;
            (host, path)
        };

        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().ok()),
            None => (host_port, None),
        };
        if host.is_empty() {
            return None;
        }

        let path = path
            .trim_matches('/')
            .strip_suffix(".git")
            .unwrap_or_else(|| path.trim_matches('/'));
        let mut segments = path.split('/').filter(|segment| !segment.is_empty());
        let mut owner_segments: Vec<&str> = segments.by_ref().collect();
        let name = owner_segments.pop()?;
        if owner_segments.is_empty() || name.is_empty() {
            return None;
        }

        Some(Self {
            host: host.to_ascii_lowercase(),
            port,
            owner: owner_segments.join("/"),
            name: name.to_owned(),
        })
    }
}

/// Drops a `user@` prefix, which carries no information this layer needs: the
/// credential comes from the chain, not from the remote URL.
fn strip_userinfo(rest: &str) -> &str {
    match rest.split_once('@') {
        // Only a `@` before the path is userinfo.
        Some((userinfo, after)) if !userinfo.contains('/') => after,
        _ => rest,
    }
}
