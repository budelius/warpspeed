//! The real HTTP transport behind the forge clients and the detector.
//!
//! It carries one host's credential and will not attach it to a request for any
//! other host. That is a rule rather than a consequence: the credential chain
//! hands back whatever the machine already holds — often a broad git
//! credential — and a redirect or a mis-built URL must not be able to post it
//! somewhere it was never issued for.

use async_trait::async_trait;

use super::client::{ForgeTransport, HttpResponse};
use super::detect::ForgeProber;

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;

/// GitHub rejects a request without one, and it identifies this client in a
/// host's logs.
const USER_AGENT: &str = "WarpSpeed";

/// Whether a credential issued for `credential_host` may be attached to `url`.
///
/// Only an exact authority match qualifies, port included. A subdomain is a
/// different host and a different trust boundary, so `api.github.com` does not
/// inherit `github.com`'s credential — the caller names the host it means. A
/// host reached on a non-default port is likewise a distinct authority, so
/// `credential_host` must carry the port when the remote named one.
pub fn may_attach_credential(url: &str, credential_host: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    // A credential must never travel in clear text.
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    // `host_str` drops the port, so comparing it alone would let a credential
    // for `example.com` reach `example.com:8443`, a different service.
    let authority = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    authority.eq_ignore_ascii_case(credential_host)
}

/// Talks to one forge host.
pub struct HttpForgeTransport {
    client: http_client::Client,
    /// The host whose credential this transport may send, including the port
    /// when the remote named one.
    credential_host: String,
}

impl HttpForgeTransport {
    pub fn new(client: http_client::Client, credential_host: impl Into<String>) -> Self {
        Self {
            client,
            credential_host: credential_host.into(),
        }
    }
}

#[async_trait]
impl ForgeTransport for HttpForgeTransport {
    async fn get(&self, url: &str, token: Option<&str>) -> anyhow::Result<HttpResponse> {
        let mut request = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT);

        if let Some(token) = token {
            if may_attach_credential(url, &self.credential_host) {
                request = request.bearer_auth(token);
            } else {
                // Degrading to an unauthenticated request is the safe failure:
                // the caller sees "unauthenticated" rather than the token
                // reaching a host it was not issued for.
                log::warn!(
                    "forge: refusing to send the {} credential to {url}",
                    self.credential_host
                );
            }
        }

        let response = request.send().await?;
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        Ok(HttpResponse { status, body })
    }
}

#[async_trait]
impl ForgeProber for HttpForgeTransport {
    async fn responds(&self, url: &str) -> bool {
        // Probing is deliberately unauthenticated: it asks only whether a host
        // speaks a forge's API, which no credential is needed to learn.
        matches!(self.get(url, None).await, Ok(response) if response.is_success())
    }
}
