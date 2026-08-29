//! The [`ansi::Handler`] implementation for [`HeadlessScreen`].
//!
//! Nearly every method forwards to whichever grid is currently active. The two
//! that do not are `set_mode` and `unset_mode`, which intercept
//! [`ansi::Mode::SwapScreen`] -- `GridHandler` panics on it with
//! "Handled in model layer", because in the app the swap is owned by
//! `TerminalModel`, which holds both screens. The supervisor has no
//! `TerminalModel`, so it owns the swap here instead.
//!
//! This file is mechanically derived from the trait definition in
//! `model/ansi/handler.rs`. When a required method is added there, the compiler
//! fails here; regenerate rather than hand-patching.

use std::io;

use warpui_core::color::ColorU;

use super::screen::HeadlessScreen;
use crate::model::ansi;
use crate::model::ansi::*;
use crate::model::index::VisibleRow;
use crate::model::selection::ScrollDelta;
use crate::model::{KeyboardModes, KeyboardModesApplyBehavior};

impl ansi::Handler for HeadlessScreen {
    /// Intercepts the alternate-screen switch; everything else goes to the
    /// active grid.
    fn set_mode(&mut self, mode: ansi::Mode) {
        if let ansi::Mode::SwapScreen {
            save_cursor_and_clear_screen,
        } = mode
        {
            self.enter_alternate_screen(save_cursor_and_clear_screen);
            return;
        }
        self.active_grid_mut().set_mode(mode)
    }

    /// The counterpart to [`Self::set_mode`].
    fn unset_mode(&mut self, mode: ansi::Mode) {
        if let ansi::Mode::SwapScreen {
            save_cursor_and_clear_screen,
        } = mode
        {
            self.exit_alternate_screen(save_cursor_and_clear_screen);
            return;
        }
        self.active_grid_mut().unset_mode(mode)
    }

    fn set_title(&mut self, a0: Option<String>) {
        self.active_grid_mut().set_title(a0)
    }
    fn set_cursor_style(&mut self, a0: Option<CursorStyle>) {
        self.active_grid_mut().set_cursor_style(a0)
    }
    fn set_cursor_shape(&mut self, a0: CursorShape) {
        self.active_grid_mut().set_cursor_shape(a0)
    }
    fn input(&mut self, a0: char) {
        self.active_grid_mut().input(a0)
    }
    fn goto(&mut self, a0: VisibleRow, a1: usize) {
        self.active_grid_mut().goto(a0, a1)
    }
    fn goto_line(&mut self, a0: VisibleRow) {
        self.active_grid_mut().goto_line(a0)
    }
    fn goto_col(&mut self, a0: usize) {
        self.active_grid_mut().goto_col(a0)
    }
    fn insert_blank(&mut self, a0: usize) {
        self.active_grid_mut().insert_blank(a0)
    }
    fn move_up(&mut self, a0: usize) {
        self.active_grid_mut().move_up(a0)
    }
    fn move_down(&mut self, a0: usize) {
        self.active_grid_mut().move_down(a0)
    }
    fn identify_terminal<W: io::Write>(&mut self, a0: &mut W, a1: Option<char>) {
        self.active_grid_mut().identify_terminal(a0, a1)
    }
    fn report_xtversion<W: io::Write>(&mut self, a0: &mut W) {
        self.active_grid_mut().report_xtversion(a0)
    }
    fn device_status<W: io::Write>(&mut self, a0: &mut W, a1: usize) {
        self.active_grid_mut().device_status(a0, a1)
    }
    fn move_forward(&mut self, a0: usize) {
        self.active_grid_mut().move_forward(a0)
    }
    fn move_backward(&mut self, a0: usize) {
        self.active_grid_mut().move_backward(a0)
    }
    fn move_down_and_cr(&mut self, a0: usize) {
        self.active_grid_mut().move_down_and_cr(a0)
    }
    fn move_up_and_cr(&mut self, a0: usize) {
        self.active_grid_mut().move_up_and_cr(a0)
    }
    fn put_tab(&mut self, a0: u16) {
        self.active_grid_mut().put_tab(a0)
    }
    fn backspace(&mut self) {
        self.active_grid_mut().backspace()
    }
    fn carriage_return(&mut self) {
        self.active_grid_mut().carriage_return()
    }
    fn linefeed(&mut self) -> ScrollDelta {
        self.active_grid_mut().linefeed()
    }
    fn bell(&mut self) {
        self.active_grid_mut().bell()
    }
    fn substitute(&mut self) {
        self.active_grid_mut().substitute()
    }
    fn newline(&mut self) {
        self.active_grid_mut().newline()
    }
    fn set_horizontal_tabstop(&mut self) {
        self.active_grid_mut().set_horizontal_tabstop()
    }
    fn scroll_up(&mut self, a0: usize) -> ScrollDelta {
        self.active_grid_mut().scroll_up(a0)
    }
    fn scroll_down(&mut self, a0: usize) -> ScrollDelta {
        self.active_grid_mut().scroll_down(a0)
    }
    fn insert_blank_lines(&mut self, a0: usize) -> ScrollDelta {
        self.active_grid_mut().insert_blank_lines(a0)
    }
    fn delete_lines(&mut self, a0: usize) -> ScrollDelta {
        self.active_grid_mut().delete_lines(a0)
    }
    fn erase_chars(&mut self, a0: usize) {
        self.active_grid_mut().erase_chars(a0)
    }
    fn delete_chars(&mut self, a0: usize) {
        self.active_grid_mut().delete_chars(a0)
    }
    fn move_backward_tabs(&mut self, a0: u16) {
        self.active_grid_mut().move_backward_tabs(a0)
    }
    fn move_forward_tabs(&mut self, a0: u16) {
        self.active_grid_mut().move_forward_tabs(a0)
    }
    fn save_cursor_position(&mut self) {
        self.active_grid_mut().save_cursor_position()
    }
    fn restore_cursor_position(&mut self) {
        self.active_grid_mut().restore_cursor_position()
    }
    fn clear_line(&mut self, a0: LineClearMode) {
        self.active_grid_mut().clear_line(a0)
    }
    fn clear_screen(&mut self, a0: ClearMode) {
        self.active_grid_mut().clear_screen(a0)
    }
    fn clear_tabs(&mut self, a0: TabulationClearMode) {
        self.active_grid_mut().clear_tabs(a0)
    }
    fn reset_state(&mut self) {
        self.active_grid_mut().reset_state()
    }
    fn reverse_index(&mut self) -> ScrollDelta {
        self.active_grid_mut().reverse_index()
    }
    fn terminal_attribute(&mut self, a0: Attr) {
        self.active_grid_mut().terminal_attribute(a0)
    }
    fn set_keyboard_enhancement_flags(&mut self, a0: KeyboardModes, a1: KeyboardModesApplyBehavior) {
        self.active_grid_mut().set_keyboard_enhancement_flags(a0, a1)
    }
    fn push_keyboard_enhancement_flags(&mut self, a0: KeyboardModes) {
        self.active_grid_mut().push_keyboard_enhancement_flags(a0)
    }
    fn pop_keyboard_enhancement_flags(&mut self, a0: u16) {
        self.active_grid_mut().pop_keyboard_enhancement_flags(a0)
    }
    fn query_keyboard_enhancement_flags<W: io::Write>(&mut self, a0: &mut W) {
        self.active_grid_mut().query_keyboard_enhancement_flags(a0)
    }
    fn set_scrolling_region(&mut self, a0: usize, a1: Option<usize>) {
        self.active_grid_mut().set_scrolling_region(a0, a1)
    }
    fn set_keypad_application_mode(&mut self) {
        self.active_grid_mut().set_keypad_application_mode()
    }
    fn unset_keypad_application_mode(&mut self) {
        self.active_grid_mut().unset_keypad_application_mode()
    }
    fn set_active_charset(&mut self, a0: CharsetIndex) {
        self.active_grid_mut().set_active_charset(a0)
    }
    fn configure_charset(&mut self, a0: CharsetIndex, a1: StandardCharset) {
        self.active_grid_mut().configure_charset(a0, a1)
    }
    fn set_color(&mut self, a0: usize, a1: ColorU) {
        self.active_grid_mut().set_color(a0, a1)
    }
    fn dynamic_color_sequence<W: io::Write>(&mut self, a0: &mut W, a1: u8, a2: usize, a3: &str) {
        self.active_grid_mut().dynamic_color_sequence(a0, a1, a2, a3)
    }
    fn reset_color(&mut self, a0: usize) {
        self.active_grid_mut().reset_color(a0)
    }
    fn clipboard_store(&mut self, a0: u8, a1: &[u8]) {
        self.active_grid_mut().clipboard_store(a0, a1)
    }
    fn clipboard_load(&mut self, a0: u8, a1: &str) {
        self.active_grid_mut().clipboard_load(a0, a1)
    }
    fn decaln(&mut self) {
        self.active_grid_mut().decaln()
    }
    fn push_title(&mut self) {
        self.active_grid_mut().push_title()
    }
    fn pop_title(&mut self) {
        self.active_grid_mut().pop_title()
    }
    fn text_area_size_pixels<W: io::Write>(&mut self, a0: &mut W) {
        self.active_grid_mut().text_area_size_pixels(a0)
    }
    fn text_area_size_chars<W: io::Write>(&mut self, a0: &mut W) {
        self.active_grid_mut().text_area_size_chars(a0)
    }
}
