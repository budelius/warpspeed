//! Working out which forge software a host runs.
//!
//! A hostname does not say what is behind it. `github.com` and `gitlab.com` are
//! known, but GitHub Enterprise, self-hosted GitLab and Gitea all live on
//! arbitrary names, so they are discovered by asking each forge's version
//! endpoint and remembering the answer.
//!
//! A host that answers nothing is remembered too, with a TTL. That is the point
//! of this cache rather than an afterthought: an operation that cannot succeed,
//! re-driven on every branch change, is exactly what produced the
//! 12,591-warning storm in the prompt chips.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use instant::Instant;
use parking_lot::Mutex;

use super::ForgeKind;

#[cfg(test)]
#[path = "detect_tests.rs"]
mod tests;

/// How long a "this host is not a forge we know" answer stands before it is
/// worth asking again.
pub const DEFAULT_NEGATIVE_TTL: Duration = Duration::from_secs(60 * 60);

/// Asks whether a URL answers.
///
/// The seam exists so detection is testable without a network: a stub decides
/// which endpoints respond.
#[async_trait]
pub trait ForgeProber: Send + Sync {
    /// Whether `url` responded affirmatively — a 2xx from a version endpoint.
    async fn responds(&self, url: &str) -> bool;
}

/// The version endpoints to try, in order, for a host.
///
/// GitLab first because its endpoint is the most specific; Gitea next; GitHub
/// Enterprise last, because `/api/v3/` is the vaguest of the three and most
/// likely to be answered by something that is not GitHub.
pub fn probe_urls(authority: &str) -> Vec<(ForgeKind, String)> {
    vec![
        (
            ForgeKind::GitLab,
            format!("https://{authority}/api/v4/version"),
        ),
        (
            ForgeKind::Gitea,
            format!("https://{authority}/api/v1/version"),
        ),
        (ForgeKind::GitHub, format!("https://{authority}/api/v3/")),
    ]
}

/// `host` with its port when the remote named one.
pub fn authority(host: &str, port: Option<u16>) -> String {
    match port {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

#[derive(Clone, Copy)]
struct CacheEntry {
    kind: Option<ForgeKind>,
    recorded: Instant,
}

/// Resolves a host to a forge, once, and remembers it.
pub struct ForgeDetector {
    prober: Box<dyn ForgeProber>,
    cache: Mutex<HashMap<String, CacheEntry>>,
    negative_ttl: Duration,
}

impl ForgeDetector {
    pub fn new(prober: Box<dyn ForgeProber>, negative_ttl: Duration) -> Self {
        Self {
            prober,
            cache: Mutex::new(HashMap::new()),
            negative_ttl,
        }
    }

    /// The forge behind `host`, probing only if the answer is not already known.
    ///
    /// `None` means no forge answered. That is a settled answer for the TTL, not
    /// a failure to retry.
    pub async fn detect(&self, host: &str, port: Option<u16>) -> Option<ForgeKind> {
        // A public host needs no request at all.
        if let Some(known) = ForgeKind::for_known_host(host) {
            return Some(known);
        }

        let authority = authority(host, port);
        if let Some(entry) = self.cached(&authority) {
            return entry;
        }

        let mut detected = None;
        for (kind, url) in probe_urls(&authority) {
            if self.prober.responds(&url).await {
                detected = Some(kind);
                break;
            }
        }

        self.cache.lock().insert(
            authority,
            CacheEntry {
                kind: detected,
                recorded: Instant::now(),
            },
        );
        detected
    }

    /// The cached answer for `authority`, if one still stands.
    ///
    /// A positive answer never expires — forge software does not change under a
    /// hostname within a session. A negative one does, so a host that was down
    /// or unreachable gets another chance.
    fn cached(&self, authority: &str) -> Option<Option<ForgeKind>> {
        let cache = self.cache.lock();
        let entry = cache.get(authority)?;
        match entry.kind {
            Some(kind) => Some(Some(kind)),
            None if entry.recorded.elapsed() < self.negative_ttl => Some(None),
            None => None,
        }
    }
}
