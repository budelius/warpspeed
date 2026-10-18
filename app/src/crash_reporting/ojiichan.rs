use std::panic::PanicHookInfo;
use std::sync::Arc;
use std::time::Duration;

use sentry::integrations::backtrace::{
    AttachStacktraceIntegration, ProcessStacktraceIntegration, current_stacktrace,
};
use sentry::integrations::panic::PanicIntegration;
use sentry::protocol::{EnvelopeItem, Event, Exception, Frame, Stacktrace};
use sentry::{ClientOptions, Envelope, Level, Transport, TransportFactory};
use warp_errors::REPORT_ERROR_LOCATION;

const PANIC_LOCATION: &str = "rust_panic_location";

pub(super) fn configure(options: &mut ClientOptions) {
    // The SDK's default panic integration takes precedence over custom extractors if both exist.
    options.default_integrations = false;
    options.integrations = vec![
        Arc::new(AttachStacktraceIntegration),
        Arc::new(PanicIntegration::new().add_extractor(|info| Some(panic_event(info)))),
        Arc::new(ProcessStacktraceIntegration),
    ];
    options.transport = Some(Arc::new(transport));
    options.before_breadcrumb = Some(Arc::new(|_| None));
    options.before_send = Some(Arc::new(|event| Some(sanitize(event))));
    options.attach_stacktrace = true;
    options.auto_session_tracking = false;
    options.send_default_pii = false;
    options.shutdown_timeout = Duration::from_secs(2);
}

fn panic_event(info: &PanicHookInfo<'_>) -> Event<'static> {
    let mut stacktrace = current_stacktrace().unwrap_or_default();
    // Release builds may lack debug symbols, but Rust still supplies the panic's source location.
    if let Some(location) = info.location() {
        stacktrace.frames.push(Frame {
            function: Some(PANIC_LOCATION.into()),
            filename: Some(location.file().into()),
            lineno: Some(location.line().into()),
            colno: Some(location.column().into()),
            in_app: Some(true),
            ..Default::default()
        });
    }
    Event {
        level: Level::Fatal,
        exception: vec![Exception {
            ty: "panic".into(),
            stacktrace: Some(stacktrace),
            ..Default::default()
        }]
        .into(),
        ..Default::default()
    }
}

fn transport(options: &ClientOptions) -> Arc<dyn Transport> {
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
            .map(sanitize_exception)
            .collect(),
        stacktrace: event.stacktrace.map(sanitize_stacktrace),
        ..Default::default()
    }
}

fn sanitize_exception(exception: Exception) -> Exception {
    // Error types are inferred from Debug output by the SDK, not always static names.
    let is_panic = exception.ty == "panic";
    let stacktrace = exception.stacktrace.map(sanitize_stacktrace);
    let value = stacktrace.as_ref().and_then(|stacktrace| {
        let marker = if is_panic {
            PANIC_LOCATION
        } else {
            REPORT_ERROR_LOCATION
        };
        let frame = stacktrace
            .frames
            .iter()
            .rev()
            .find(|frame| frame.function.as_deref() == Some(marker))?;
        let filename = frame.filename.as_deref()?;
        let line = frame.lineno?;
        let column = frame.colno?;
        if is_panic {
            Some(format!("{filename}:{line}:{column}"))
        } else {
            Some(format!(
                "{}:{filename}:{line}:{column}",
                report_component(filename)
            ))
        }
    });
    Exception {
        ty: if is_panic { "panic" } else { "Error" }.into(),
        value,
        stacktrace,
        ..Default::default()
    }
}

fn report_component(filename: &str) -> &'static str {
    match filename {
        "auth_manager.rs" | "auth_state.rs" | "session.rs" => "authentication",
        "listener.rs" | "update_manager.rs" => "cloud-objects",
        "terminal_model.rs" | "ansi_handler.rs" | "grid_handler.rs" | "event_loop.rs" => "terminal",
        "workspace.rs" | "view.rs" | "root_view.rs" => "workspace",
        "driver.rs" | "agent.rs" => "agent",
        "ojiichan.rs" | "crash_reporting.rs" => "crash-reporting",
        "sqlite.rs" | "session_store.rs" => "persistence",
        _ => "application",
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
