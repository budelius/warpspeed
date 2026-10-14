use std::sync::Arc;
use std::time::Duration;

use sentry::protocol::{EnvelopeItem, Event, Exception, Frame, Stacktrace};
use sentry::{ClientOptions, Envelope, Transport, TransportFactory};

pub(super) fn transport(options: &ClientOptions) -> Arc<dyn Transport> {
    Arc::new(DiagnosticsTransport(
        sentry::transports::DefaultTransportFactory.create_transport(options),
    ))
}

struct DiagnosticsTransport(Arc<dyn Transport>);

impl Transport for DiagnosticsTransport {
    fn send_envelope(&self, envelope: Envelope) {
        // Attachments bypass before_send and can include process samples or heap profiles.
        if let Some(envelope) = envelope.filter(|item| matches!(item, EnvelopeItem::Event(_))) {
            self.0.send_envelope(envelope);
        }
    }

    fn flush(&self, timeout: Duration) -> bool {
        self.0.flush(timeout)
    }

    fn shutdown(&self, timeout: Duration) -> bool {
        self.0.shutdown(timeout)
    }
}

/// Export only code locations and build metadata, never captured terminal or request data.
pub(super) fn sanitize(event: Event<'static>) -> Event<'static> {
    Event {
        event_id: event.event_id,
        timestamp: event.timestamp,
        level: event.level,
        release: event.release,
        environment: event.environment,
        platform: event.platform,
        message: Some(
            if event.message.as_deref() == Some("WarpSpeed interrupted session recovered") {
                "WarpSpeed interrupted session recovered".to_owned()
            } else {
                "WarpSpeed error; diagnostic details retained locally".to_owned()
            },
        ),
        exception: event
            .exception
            .into_iter()
            .map(|exception| Exception {
                // Error types are inferred from Debug output by the SDK, not always static names.
                ty: if exception.ty == "panic" {
                    "panic"
                } else {
                    "Error"
                }
                .to_owned(),
                stacktrace: exception.stacktrace.map(sanitize_stacktrace),
                ..Default::default()
            })
            .collect(),
        stacktrace: event.stacktrace.map(sanitize_stacktrace),
        ..Default::default()
    }
}

fn sanitize_stacktrace(stacktrace: Stacktrace) -> Stacktrace {
    Stacktrace {
        frames: stacktrace
            .frames
            .into_iter()
            .map(|frame| Frame {
                function: frame.function,
                symbol: frame.symbol,
                module: frame.module,
                filename: frame
                    .filename
                    .and_then(|filename| filename.rsplit(['/', '\\']).next().map(str::to_owned)),
                lineno: frame.lineno,
                colno: frame.colno,
                in_app: frame.in_app,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

#[cfg(test)]
#[path = "ojiichan_tests.rs"]
mod tests;
