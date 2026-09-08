use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::executor::block_on;

use super::{DEFAULT_NEGATIVE_TTL, ForgeDetector, ForgeProber, authority, probe_urls};
use crate::forge::ForgeKind;

/// Answers for one endpoint and counts every question asked, so a test can
/// assert the cache actually spared a request.
struct StubProber {
    answers_for: Option<&'static str>,
    calls: Arc<AtomicUsize>,
}

impl StubProber {
    fn new(answers_for: Option<&'static str>) -> (Box<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Box::new(Self {
                answers_for,
                calls: calls.clone(),
            }),
            calls,
        )
    }
}

#[async_trait]
impl ForgeProber for StubProber {
    async fn responds(&self, url: &str) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.answers_for == Some(url)
    }
}

#[test]
fn authority_appends_a_port_only_when_there_is_one() {
    assert_eq!(authority("example.com", None), "example.com");
    assert_eq!(authority("example.com", Some(8443)), "example.com:8443");
}

#[test]
fn probe_urls_ask_gitlab_then_gitea_then_github() {
    let urls = probe_urls("git.example.com");
    let kinds: Vec<ForgeKind> = urls.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(
        kinds,
        vec![ForgeKind::GitLab, ForgeKind::Gitea, ForgeKind::GitHub]
    );
    assert_eq!(urls[0].1, "https://git.example.com/api/v4/version");
    assert_eq!(urls[1].1, "https://git.example.com/api/v1/version");
    assert_eq!(urls[2].1, "https://git.example.com/api/v3/");
}

/// A public host is knowable by name, so detection must not reach the network
/// for it at all.
#[test]
fn a_known_host_is_resolved_without_probing() {
    let (prober, calls) = StubProber::new(None);
    let detector = ForgeDetector::new(prober, DEFAULT_NEGATIVE_TTL);

    assert_eq!(
        block_on(detector.detect("github.com", None)),
        Some(ForgeKind::GitHub)
    );
    assert_eq!(
        block_on(detector.detect("gitlab.com", None)),
        Some(ForgeKind::GitLab)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no request should be made");
}

#[test]
fn a_self_hosted_gitlab_is_found_by_its_version_endpoint() {
    let (prober, _) = StubProber::new(Some("https://git.example.com/api/v4/version"));
    let detector = ForgeDetector::new(prober, DEFAULT_NEGATIVE_TTL);

    assert_eq!(
        block_on(detector.detect("git.example.com", None)),
        Some(ForgeKind::GitLab)
    );
}

#[test]
fn a_gitea_host_is_found_by_its_version_endpoint() {
    let (prober, _) = StubProber::new(Some("https://git.example.com/api/v1/version"));
    let detector = ForgeDetector::new(prober, DEFAULT_NEGATIVE_TTL);

    assert_eq!(
        block_on(detector.detect("git.example.com", None)),
        Some(ForgeKind::Gitea)
    );
}

#[test]
fn a_github_enterprise_host_is_found_by_its_api_root() {
    let (prober, _) = StubProber::new(Some("https://git.example.com/api/v3/"));
    let detector = ForgeDetector::new(prober, DEFAULT_NEGATIVE_TTL);

    assert_eq!(
        block_on(detector.detect("git.example.com", None)),
        Some(ForgeKind::GitHub)
    );
}

#[test]
fn a_port_from_the_remote_is_carried_into_the_probe() {
    let (prober, _) = StubProber::new(Some("https://git.example.com:8443/api/v1/version"));
    let detector = ForgeDetector::new(prober, DEFAULT_NEGATIVE_TTL);

    assert_eq!(
        block_on(detector.detect("git.example.com", Some(8443))),
        Some(ForgeKind::Gitea)
    );
}

#[test]
fn a_positive_answer_is_cached() {
    let (prober, calls) = StubProber::new(Some("https://git.example.com/api/v4/version"));
    let detector = ForgeDetector::new(prober, DEFAULT_NEGATIVE_TTL);

    assert_eq!(
        block_on(detector.detect("git.example.com", None)),
        Some(ForgeKind::GitLab)
    );
    let after_first = calls.load(Ordering::SeqCst);
    assert_eq!(
        block_on(detector.detect("git.example.com", None)),
        Some(ForgeKind::GitLab)
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        after_first,
        "a known answer must not be asked for again"
    );
}

/// The negative is the one that matters. Without it, a host that answers
/// nothing is re-probed on every branch change.
#[test]
fn a_host_that_answers_nothing_is_not_probed_again_within_the_ttl() {
    let (prober, calls) = StubProber::new(None);
    let detector = ForgeDetector::new(prober, DEFAULT_NEGATIVE_TTL);

    assert_eq!(block_on(detector.detect("git.example.com", None)), None);
    let after_first = calls.load(Ordering::SeqCst);
    assert_eq!(after_first, 3, "all three endpoints should have been tried");

    assert_eq!(block_on(detector.detect("git.example.com", None)), None);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        after_first,
        "the negative must be cached, not re-probed"
    );
}

/// The negative expires, so a host that was merely down gets another chance.
#[test]
fn a_negative_answer_is_retried_once_the_ttl_has_passed() {
    let (prober, calls) = StubProber::new(None);
    let detector = ForgeDetector::new(prober, Duration::ZERO);

    assert_eq!(block_on(detector.detect("git.example.com", None)), None);
    let after_first = calls.load(Ordering::SeqCst);

    assert_eq!(block_on(detector.detect("git.example.com", None)), None);
    assert!(
        calls.load(Ordering::SeqCst) > after_first,
        "an expired negative must be probed again"
    );
}

/// Two hosts are cached separately; one answering must not settle the other.
#[test]
fn hosts_are_cached_independently() {
    let (prober, _) = StubProber::new(Some("https://a.example.com/api/v4/version"));
    let detector = ForgeDetector::new(prober, DEFAULT_NEGATIVE_TTL);

    assert_eq!(
        block_on(detector.detect("a.example.com", None)),
        Some(ForgeKind::GitLab)
    );
    assert_eq!(block_on(detector.detect("b.example.com", None)), None);
}
