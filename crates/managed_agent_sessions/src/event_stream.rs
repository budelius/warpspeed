use std::io::{BufRead, BufReader, Read};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use instant::Instant;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde::Deserialize;

use super::{
    EnvironmentProfile, EventPage, fetch_event_page, http_error, validate_request_destination,
};

const MAX_FRAME_BYTES: u64 = 1_048_576;
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// Follows the SSE contract, retaining polling compatibility with older dashboards and proxies.
pub(super) struct EventFeed {
    client: Client,
    stream: Option<BufReader<Response>>,
    retry_at: Option<Instant>,
    unsupported: bool,
}

impl EventFeed {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            // A finite lifetime rotates credentials and bounds disconnect detection, even if the
            // peer stops sending keepalives. Redirects must never forward a bearer to another host.
            client: Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(60))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            stream: None,
            retry_at: None,
            unsupported: false,
        })
    }

    pub(super) fn next(
        &mut self,
        profile: &EnvironmentProfile,
        job_id: &str,
        token: &str,
        cursor: Option<&str>,
        wait_ms: u64,
    ) -> Result<(EventPage, bool)> {
        if !self.unsupported && self.retry_at.is_none_or(|at| Instant::now() >= at) {
            if self.stream.is_none() {
                let mut request = self
                    .client
                    .get(format!(
                        "{}/api/agent-control/runs/{job_id}/events/stream",
                        profile.base_url
                    ))
                    .bearer_auth(token)
                    .header("X-Stozn-Client", "warpspeed")
                    .header("Accept", "text/event-stream");
                if let Some(cursor) = cursor {
                    request = request.header("Last-Event-ID", cursor);
                }
                validate_request_destination(profile, &request)?;
                if let Ok(response) = request.send() {
                    let status = response.status();
                    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                        return Err(http_error(response));
                    }
                    let is_stream = response
                        .headers()
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        .is_some_and(|value| value.split(';').next() == Some("text/event-stream"));
                    if status.is_success() && is_stream {
                        self.stream = Some(BufReader::new(response));
                    } else {
                        self.unsupported = matches!(
                            status,
                            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
                        ) || status.is_success();
                    }
                }
            }
            if let Some(stream) = self.stream.as_mut() {
                match read_frame(stream).and_then(|frame| frame.page(cursor)) {
                    Ok(Some(page)) => {
                        if page.gap || page.terminal {
                            self.stream = None;
                        }
                        return Ok((page, false));
                    }
                    Ok(None) => {
                        self.stream = None;
                        return Ok((empty_page(cursor), false));
                    }
                    Err(_) => self.stream = None,
                }
            }
            self.retry_at = Some(Instant::now() + RETRY_INTERVAL);
        }
        let (mut page, gap) =
            fetch_event_page(&self.client, profile, job_id, token, cursor, wait_ms)?;
        // Polling marks every page of a finished run terminal, including pages with more to drain.
        page.terminal &= page.items.is_empty();
        Ok((page, gap))
    }
}

#[derive(Default)]
struct Frame {
    kind: String,
    id: Option<String>,
    data: String,
}

impl Frame {
    fn page(self, cursor: Option<&str>) -> Result<Option<EventPage>> {
        let mut page = empty_page(cursor);
        match self.kind.as_str() {
            "run_event" => {
                let id = self
                    .id
                    .filter(|id| !id.is_empty())
                    .context("event stream omitted its cursor")?;
                page.items
                    .push(serde_json::from_str(&self.data).context("invalid streamed run event")?);
                page.next_cursor = Some(id);
            }
            "gap" => {
                #[derive(Deserialize)]
                struct Gap {
                    earliest_cursor: Option<String>,
                }
                let gap: Gap = serde_json::from_str(&self.data)?;
                page.next_cursor = gap.earliest_cursor.clone();
                page.earliest_cursor = gap.earliest_cursor;
                page.gap = true;
            }
            "terminal" => page.terminal = true,
            "reauth" => return Ok(None),
            "error" => bail!("event stream unavailable"),
            // Keepalive comments and future event kinds do not advance the applied cursor.
            _ => {}
        }
        Ok(Some(page))
    }
}

fn empty_page(cursor: Option<&str>) -> EventPage {
    EventPage {
        items: Vec::new(),
        next_cursor: cursor.map(str::to_owned),
        terminal: false,
        gap: false,
        earliest_cursor: None,
    }
}

fn read_frame(reader: &mut impl BufRead) -> Result<Frame> {
    let mut frame = Frame::default();
    let mut remaining = MAX_FRAME_BYTES;
    loop {
        let mut line = String::new();
        let read = reader.take(remaining + 1).read_line(&mut line)?;
        if read == 0 {
            bail!("event stream closed");
        }
        remaining = remaining
            .checked_sub(read as u64)
            .context("event stream frame too large")?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return Ok(frame);
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => frame.kind = value.to_owned(),
            "id" if !value.contains('\0') => frame.id = Some(value.to_owned()),
            "data" => {
                if !frame.data.is_empty() {
                    frame.data.push('\n');
                }
                frame.data.push_str(value);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "event_stream_tests.rs"]
mod tests;
