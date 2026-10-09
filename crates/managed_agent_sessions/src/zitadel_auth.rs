use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use instant::Instant;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::StoznIdentity;

const REFRESH_WINDOW_SECONDS: i64 = 60;
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Compiled-in OIDC settings for one environment's Zitadel instance.
pub(crate) struct ZitadelProfile {
    pub issuer: &'static str,
    pub client_id: &'static str,
    pub project_id: &'static str,
}

impl ZitadelProfile {
    fn scope(&self) -> String {
        format!(
            "openid profile email offline_access urn:zitadel:iam:org:project:id:{}:aud urn:zitadel:iam:org:projects:roles",
            self.project_id
        )
    }

    pub(crate) fn issuer_host(&self) -> &str {
        self.issuer
            .split_once("://")
            .map_or(self.issuer, |(_, rest)| rest.trim_end_matches('/'))
    }
}

pub(crate) struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_at: DateTime<Utc>,
    pub interval: Duration,
}

#[derive(Debug)]
pub(crate) struct TokenSet {
    pub access_token: String,
    pub access_expires_at: DateTime<Utc>,
    pub refresh_token: String,
}

#[derive(Deserialize)]
struct DeviceAuthorizationResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: i64,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
}

pub(crate) fn start_device_login(profile: &ZitadelProfile) -> Result<DeviceStart> {
    let response = Client::new()
        .post(format!("{}/oauth/v2/device_authorization", profile.issuer))
        .form(&[
            ("client_id", profile.client_id),
            ("scope", profile.scope().as_str()),
        ])
        .send()
        .context("could not reach the STOZN sign-in service")?;
    if !response.status().is_success() {
        bail!(
            "STOZN sign-in service refused the login request ({})",
            response.status()
        );
    }
    let started: DeviceAuthorizationResponse =
        response.json().context("invalid STOZN sign-in response")?;
    let verification_url = started
        .verification_uri_complete
        .unwrap_or(started.verification_uri);
    validate_issuer_url(profile, &verification_url)?;
    if started.expires_in <= 0
        || started.device_code.is_empty()
        || !valid_user_code(&started.user_code)
    {
        bail!("STOZN sign-in service returned an invalid login request");
    }
    Ok(DeviceStart {
        device_code: started.device_code,
        user_code: started.user_code,
        verification_url,
        expires_at: Utc::now() + chrono::Duration::seconds(started.expires_in),
        interval: Duration::from_secs(started.interval.unwrap_or(5).clamp(1, 30)),
    })
}

/// Polls the token endpoint per RFC 8628 until approval, denial, expiry or `max_wait`.
pub(crate) fn poll_device_login(
    profile: &ZitadelProfile,
    device_code: &str,
    interval: Duration,
    max_wait: Duration,
) -> Result<TokenSet> {
    let started = Instant::now();
    let mut interval = interval;
    loop {
        if started.elapsed() >= max_wait {
            bail!("STOZN login request expired");
        }
        let response = Client::new()
            .post(format!("{}/oauth/v2/token", profile.issuer))
            .form(&[
                ("client_id", profile.client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ])
            .send()
            .context("STOZN login polling failed")?;
        if response.status().is_success() {
            return token_set(
                response.json().context("invalid STOZN token response")?,
                None,
            );
        }
        let error: TokenError = response.json().context("invalid STOZN token error")?;
        match error.error.as_str() {
            "authorization_pending" => {}
            "slow_down" => interval = (interval + Duration::from_secs(5)).min(MAX_POLL_INTERVAL),
            "access_denied" => bail!("STOZN login was denied in the browser"),
            "expired_token" => bail!("STOZN login request expired"),
            other => bail!("STOZN login failed: {other}"),
        }
        thread::sleep(interval);
    }
}

pub(crate) fn refresh(profile: &ZitadelProfile, refresh_token: &str) -> Result<TokenSet> {
    let response = Client::new()
        .post(format!("{}/oauth/v2/token", profile.issuer))
        .form(&[
            ("client_id", profile.client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .context("could not reach the STOZN sign-in service")?;
    let status = response.status();
    if !status.is_success() {
        let error = response.json::<TokenError>().ok();
        if matches!(status.as_u16(), 400 | 401)
            && let Some(error) = error.filter(|error| error.error == "invalid_grant")
        {
            bail!(RefreshRejected(error.error));
        }
        bail!("STOZN sign-in service could not refresh the sign-in ({status})");
    }
    token_set(
        response.json().context("invalid STOZN token response")?,
        Some(refresh_token),
    )
}

/// The token endpoint rejected a refresh; the stored sign-in is no longer usable.
#[derive(Debug)]
pub(crate) struct RefreshRejected(pub String);

impl std::fmt::Display for RefreshRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "STOZN sign-in expired ({}); sign in again", self.0)
    }
}

impl std::error::Error for RefreshRejected {}

fn token_set(response: TokenResponse, previous_refresh: Option<&str>) -> Result<TokenSet> {
    let refresh_token = response
        .refresh_token
        .or_else(|| previous_refresh.map(str::to_owned))
        .context("STOZN sign-in did not return a refresh token")?;
    if response.access_token.is_empty() || response.expires_in <= 0 {
        bail!("STOZN sign-in returned an invalid access token");
    }
    Ok(TokenSet {
        access_token: response.access_token,
        access_expires_at: Utc::now() + chrono::Duration::seconds(response.expires_in),
        refresh_token,
    })
}

pub(crate) fn needs_refresh(access_expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    access_expires_at - now < chrono::Duration::seconds(REFRESH_WINDOW_SECONDS)
}

/// Reads identity and agent-platform role from the access token's payload. The token was just
/// received from the issuer over TLS; resource servers verify its signature.
pub(crate) fn identity_from_access_token(
    access_token: &str,
    project_id: &str,
    issuer_host: &str,
) -> Result<StoznIdentity> {
    let payload = access_token
        .split('.')
        .nth(1)
        .context("STOZN sign-in returned a malformed access token")?;
    let claims: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(payload)
            .context("STOZN sign-in returned a malformed access token")?,
    )
    .context("STOZN sign-in returned a malformed access token")?;
    let roles: Vec<&str> = [
        format!("urn:zitadel:iam:org:project:{project_id}:roles"),
        "urn:zitadel:iam:org:project:roles".to_owned(),
    ]
    .iter()
    .filter_map(|key| claims.get(key).and_then(Value::as_object))
    .flat_map(|roles| roles.keys().map(String::as_str))
    .collect();
    let role = if roles.contains(&"agent-admin") {
        "admin"
    } else if roles.contains(&"agent-user") {
        "user"
    } else {
        bail!("Your account has no access to the agent platform ({issuer_host})");
    };
    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|sub| !sub.is_empty())
        .context("STOZN sign-in returned an access token without a subject")?;
    let display_name = ["name", "preferred_username", "email"]
        .iter()
        .find_map(|key| claims.get(*key).and_then(Value::as_str))
        .map(str::to_owned);
    Ok(StoznIdentity {
        subject: subject.to_owned(),
        role: role.to_owned(),
        display_name,
    })
}

fn validate_issuer_url(profile: &ZitadelProfile, url: &str) -> Result<()> {
    let allowed = reqwest::Url::parse(profile.issuer)?;
    let requested = reqwest::Url::parse(url).context("STOZN sign-in returned an invalid URL")?;
    let same_origin = requested.scheme() == allowed.scheme()
        && requested.host_str() == allowed.host_str()
        && requested.port_or_known_default() == allowed.port_or_known_default();
    if !same_origin
        || !requested.username().is_empty()
        || requested.password().is_some()
        || requested.fragment().is_some()
    {
        bail!("STOZN sign-in returned an unsafe login URL");
    }
    Ok(())
}

fn valid_user_code(code: &str) -> bool {
    (4..=16).contains(&code.len())
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
#[path = "zitadel_auth_tests.rs"]
mod tests;
