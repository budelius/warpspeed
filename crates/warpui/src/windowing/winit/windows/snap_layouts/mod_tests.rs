use super::*;

#[test]
fn the_button_rectangle_round_trips_through_the_shared_state() {
    let state = SnapLayoutsState::new();
    assert_eq!(
        state.button_rect(),
        None,
        "no button is published initially"
    );

    let rect = ButtonRect {
        left: 908,
        top: 0,
        right: 954,
        bottom: 35,
    };
    state.set_button_rect(Some(rect));
    assert_eq!(state.button_rect(), Some(rect));

    // Published as `None` in fullscreen and whenever the traffic lights are not rendered.
    state.set_button_rect(None);
    assert_eq!(state.button_rect(), None);
}

#[test]
fn the_hover_is_recorded_and_taken_once() {
    let state = SnapLayoutsState::new();
    assert!(!state.take_hovered(), "no hover is in effect initially");

    state.note_hovered();
    assert!(state.take_hovered(), "the recorded hover is reported once");
    assert!(!state.take_hovered(), "and is cleared by being taken");
}
