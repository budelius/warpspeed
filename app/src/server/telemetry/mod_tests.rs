use rudder_message::Track;
use virtual_fs::VirtualFS;

use super::*;

// Tests that events with UGC are not persisted to desk.
#[test]
fn test_persist_events_doesnt_include_ugc_events() {
    let telemetry_api = TelemetryApi::new();

    VirtualFS::test(
        "test_persist_events_doesnt_include_ugc_events",
        |dirs, _sandbox| {
            // The event queue is process-wide, so anything another test in this
            // binary recorded is still sitting in it. Draining first is what
            // makes the two events below the only ones this test can be
            // measuring.
            let _ = warpui::telemetry::flush_events();

            // Add one event without UGC
            let user_id = Some("user".into());
            let anonymous_id = "anonymous_id".to_owned();

            warpui::telemetry::record_event(
                user_id.clone(),
                anonymous_id.clone(),
                "non UGC event name".into(),
                None,  /* payload */
                false, /* contains_ugc  */
                warpui::time::get_current_time(),
            );

            warpui::telemetry::record_event(
                user_id.clone(),
                anonymous_id.clone(),
                "UGC event name".into(),
                None, /* payload */
                true, /* contains_ugc  */
                warpui::time::get_current_time(),
            );

            let file_path = dirs.root().join("rudderstack");

            telemetry_api
                .flush_and_persist_events_at_path(10, PrivacySettingsSnapshot::mock(), &file_path)
                .expect("Should be able to persist events");

            let file_content: Vec<RudderBatchMessage> =
                serde_json::from_reader(File::open(file_path).expect("Failed to open file"))
                    .expect("Failed to parse file");

            // Named rather than counted: the queue is process-wide, so another
            // test recording an event between the drain above and the flush
            // would break a bare count without saying anything about whether
            // UGC was excluded, which is what this test is for.
            let persisted: Vec<&str> = file_content
                .iter()
                .map(|message| message.unwrap_track().event.as_str())
                .collect();
            assert!(
                persisted.contains(&"non UGC event name"),
                "persisted events were {persisted:?}"
            );
            assert!(
                !persisted.contains(&"UGC event name"),
                "a UGC event must not be persisted; got {persisted:?}"
            );
        },
    );
}

impl RudderBatchMessage {
    fn unwrap_track(&self) -> &Track {
        match self {
            RudderBatchMessage::Track(track) => track,
            _ => panic!("Expected a track event"),
        }
    }
}
