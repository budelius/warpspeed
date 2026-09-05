use std::borrow::Cow;

use crate::SizeInfo;

/// Messages that may be sent to the `EventLoop`.
#[derive(Debug)]
pub enum Message {
    /// Data that should be written to the PTY.
    Input(Cow<'static, [u8]>),

    /// Indicates that the `EventLoop` should be shut down.
    Shutdown,

    /// Indicates that the child process has exited.
    ///
    /// Only used on Windows, as we need to pass this information to the
    /// event loop via the channel (and cannot use the child event token).
    #[cfg_attr(not(windows), allow(dead_code))]
    ChildExited,

    /// Instruction to resize the PTY.
    Resize {
        size: SizeInfo,
        /// Whether the PTY must be told this size even if it already has it.
        ///
        /// A refresh exists to re-announce a size the child may have missed —
        /// see `TerminalView::resize_alt_screen_redundantly` — so it has to go
        /// through untouched. Every other resize may be dropped by a PTY that
        /// would learn nothing from it.
        forced: bool,
    },
}
