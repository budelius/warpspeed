use std::collections::VecDeque;

use anyhow::Result;
use warp_cli::managed_sessions::ManagedEnvironment;

use super::event_stream::EventFeed;
use super::{
    EnvironmentProfile, EventPage, RunPreview, dashboard_token, profile, run_preview_event,
    validate_job_id,
};

/// A bounded live transcript. Blocking reads must run off the UI thread.
pub struct RunPreviewStream {
    profile: &'static EnvironmentProfile,
    job_id: String,
    feed: EventFeed,
    cursor: Option<String>,
    seen: VecDeque<String>,
    preview: RunPreview,
}

impl RunPreviewStream {
    pub fn new(environment: ManagedEnvironment, job_id: &str) -> Result<Self> {
        validate_job_id(job_id)?;
        Ok(Self {
            profile: profile(environment),
            job_id: job_id.to_owned(),
            feed: EventFeed::new()?,
            cursor: None,
            seen: VecDeque::new(),
            preview: RunPreview::default(),
        })
    }

    /// Waits for one event batch or keepalive and returns the latest transcript.
    pub fn read_next(&mut self) -> Result<RunPreview> {
        let token = dashboard_token()?;
        let (page, gap) = self.feed.next(
            self.profile,
            &self.job_id,
            &token,
            self.cursor.as_deref(),
            15_000,
        )?;
        self.apply(page, gap);
        Ok(self.preview.clone())
    }

    fn apply(&mut self, page: EventPage, gap: bool) {
        let empty = page.items.is_empty();
        self.preview.history_gap |= gap || page.gap;
        self.preview.terminal = page.terminal && empty;
        for event in page.items {
            if self.seen.contains(&event.event_id) {
                continue;
            }
            if let Some(event) = run_preview_event(&event) {
                self.preview.events.push(event);
            }
            self.seen.push_back(event.event_id);
            if self.seen.len() > 500 {
                self.seen.pop_front();
            }
        }
        let discard = self.preview.events.len().saturating_sub(12);
        self.preview.truncated |= discard > 0;
        self.preview.events.drain(..discard);
        self.cursor = page.next_cursor.or(self.cursor.take());
    }
}

#[cfg(test)]
#[path = "preview_stream_tests.rs"]
mod tests;
