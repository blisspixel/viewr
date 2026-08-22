//! Pure keyboard routing policy for viewer shortcuts.
//!
//! The event loop owns focus, menus, and action dispatch. This module owns only
//! which keys are consumed as viewer shortcuts versus left for widgets.

use crate::ratings::RatingAssignment;
use winit::event::ElementState;
use winit::keyboard::{Key, ModifiersState, NamedKey};

#[must_use]
pub(crate) fn single_key_shortcut_allowed(modifiers: ModifiersState) -> bool {
    !modifiers.control_key() && !modifiers.alt_key() && !modifiers.super_key()
}

/// Fullscreen toggles once for bare F or F11. Repeated press events are consumed
/// by the event loop without toggling again.
#[must_use]
pub(crate) fn is_fullscreen_toggle_key(key: &Key, modifiers: ModifiersState) -> bool {
    matches!(key, Key::Named(NamedKey::F11))
        || (single_key_shortcut_allowed(modifiers)
            && matches!(key, Key::Character(character) if character.eq_ignore_ascii_case("f")))
}

#[must_use]
pub(crate) fn rating_assignment_for_key(key: &str, repeat: bool) -> Option<RatingAssignment> {
    if repeat {
        return None;
    }
    match key {
        "0" => Some(RatingAssignment::Clear),
        "1" | "2" | "3" | "4" | "5" => {
            crate::ratings::Rating::new(key.as_bytes()[0] - b'0').map(RatingAssignment::Set)
        }
        _ => None,
    }
}

/// Crop and Spot Heal own digit keys while they are active.
#[must_use]
pub(crate) const fn rating_keys_apply(is_cropping: bool, is_healing: bool) -> bool {
    !is_cropping && !is_healing
}

/// Overlay and mode facts Escape inspects, in product order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // independent overlay and mode facts, not a flag soup
pub(crate) struct EscapeContext {
    pub context_menu_open: bool,
    pub is_cropping: bool,
    pub is_healing: bool,
    pub empty_rating_filter: bool,
    pub is_fullscreen: bool,
}

/// What Escape should do, in product order: overlay, then edit, then filter, then fullscreen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EscapeAction {
    None,
    CloseContextMenu,
    CancelCrop,
    LeaveHeal,
    ClearRatingFilter,
    LeaveFullscreen,
}

#[must_use]
pub(crate) const fn escape_action(context: EscapeContext) -> EscapeAction {
    if context.context_menu_open {
        EscapeAction::CloseContextMenu
    } else if context.is_cropping {
        EscapeAction::CancelCrop
    } else if context.is_healing {
        EscapeAction::LeaveHeal
    } else if context.empty_rating_filter {
        EscapeAction::ClearRatingFilter
    } else if context.is_fullscreen {
        EscapeAction::LeaveFullscreen
    } else {
        EscapeAction::None
    }
}

#[must_use]
pub(crate) fn is_space_key(key: &Key) -> bool {
    matches!(key, Key::Named(NamedKey::Space))
        || matches!(key, Key::Character(character) if character.as_str() == " ")
}

#[must_use]
pub(crate) fn space_release_must_unwind(key: &Key, state: ElementState, space_held: bool) -> bool {
    space_held && state == ElementState::Released && is_space_key(key)
}

/// Only the first press in one Space hold initializes temporary pan state.
#[must_use]
pub(crate) const fn space_press_starts_hold(space_held: bool) -> bool {
    !space_held
}

/// A Space tap fits in every viewing or edit mode. A completed pan or an overlay
/// that owns input suppresses Fit.
#[must_use]
pub(crate) const fn space_tap_fits(space_dragged: bool, input_owned_by_overlay: bool) -> bool {
    !space_dragged && !input_owned_by_overlay
}

#[must_use]
pub(crate) fn route_consumed_keyboard_key(
    key: &Key,
    is_cropping: bool,
    is_healing: bool,
    is_fullscreen: bool,
) -> bool {
    match key {
        Key::Character(character) => {
            let character = character.as_str();
            matches!(character, "+" | "=" | "-" | "_" | "/" | "[" | "]")
                || [
                    "o", "t", "g", "i", "r", "l", "h", "v", "s", "c", "j", "u", "f", "z", "y",
                ]
                .iter()
                .any(|shortcut| character.eq_ignore_ascii_case(shortcut))
                || (is_cropping && character.eq_ignore_ascii_case("x"))
        }
        Key::Named(
            NamedKey::ArrowRight
            | NamedKey::ArrowLeft
            | NamedKey::Home
            | NamedKey::End
            | NamedKey::PageUp
            | NamedKey::PageDown
            | NamedKey::F5
            | NamedKey::F11,
        ) => true,
        Key::Named(NamedKey::ArrowDown | NamedKey::ArrowUp) => is_cropping,
        Key::Named(NamedKey::Escape) => {
            escape_action(EscapeContext {
                context_menu_open: false,
                is_cropping,
                is_healing,
                empty_rating_filter: false,
                is_fullscreen,
            }) != EscapeAction::None
        }
        _ => false,
    }
}

#[must_use]
pub(crate) fn is_trash_shortcut_key(key: &Key) -> bool {
    matches!(key, Key::Named(NamedKey::Delete))
        || (cfg!(target_os = "macos") && matches!(key, Key::Named(NamedKey::Backspace)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumed_keyboard_routing_preserves_shortcuts_without_hijacking_controls() {
        assert!(route_consumed_keyboard_key(
            &Key::Character("t".into()),
            false,
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("+".into()),
            false,
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("j".into()),
            false,
            false,
            false,
        ));
        for key in ["b", "B", "m", "M", "x", "X"] {
            assert!(
                !route_consumed_keyboard_key(&Key::Character(key.into()), false, false, false),
                "unused culling key {key} must not be intercepted"
            );
        }
        assert!(route_consumed_keyboard_key(
            &Key::Character("x".into()),
            true,
            false,
            false,
        ));
        assert!(is_trash_shortcut_key(&Key::Named(NamedKey::Delete)));
        assert_eq!(
            is_trash_shortcut_key(&Key::Named(NamedKey::Backspace)),
            cfg!(target_os = "macos")
        );
        assert!(route_consumed_keyboard_key(
            &Key::Character("z".into()),
            false,
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("[".into()),
            false,
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("]".into()),
            false,
            false,
            false,
        ));
        assert!(single_key_shortcut_allowed(ModifiersState::default()));
        assert!(single_key_shortcut_allowed(ModifiersState::SHIFT));
        assert!(!single_key_shortcut_allowed(ModifiersState::CONTROL));
        assert!(!single_key_shortcut_allowed(ModifiersState::ALT));
        assert!(!single_key_shortcut_allowed(ModifiersState::SUPER));
    }

    #[test]
    fn fullscreen_keys_respect_modifiers_and_space_repeat_preserves_pan_state() {
        assert!(is_fullscreen_toggle_key(
            &Key::Character("f".into()),
            ModifiersState::default()
        ));
        assert!(is_fullscreen_toggle_key(
            &Key::Character("F".into()),
            ModifiersState::SHIFT
        ));
        assert!(!is_fullscreen_toggle_key(
            &Key::Character("f".into()),
            ModifiersState::CONTROL
        ));
        assert!(is_fullscreen_toggle_key(
            &Key::Named(NamedKey::F11),
            ModifiersState::CONTROL
        ));
        assert!(space_press_starts_hold(false));
        assert!(!space_press_starts_hold(true));
    }

    #[test]
    fn named_keys_include_browse_reload_fullscreen_and_escape() {
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::ArrowRight),
            true,
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::Escape),
            true,
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::Escape),
            false,
            true,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::Escape),
            false,
            false,
            true,
        ));
        assert!(!route_consumed_keyboard_key(
            &Key::Named(NamedKey::Escape),
            false,
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::ArrowRight),
            false,
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::F5),
            false,
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::F11),
            false,
            false,
            false,
        ));
        for key in [
            NamedKey::Home,
            NamedKey::End,
            NamedKey::PageUp,
            NamedKey::PageDown,
        ] {
            assert!(route_consumed_keyboard_key(
                &Key::Named(key),
                false,
                false,
                false,
            ));
        }
        assert!(!route_consumed_keyboard_key(
            &Key::Named(NamedKey::ArrowDown),
            false,
            true,
            false,
        ));
        assert!(!route_consumed_keyboard_key(
            &Key::Named(NamedKey::Enter),
            true,
            false,
            false,
        ));
        assert!(!route_consumed_keyboard_key(
            &Key::Named(NamedKey::Space),
            false,
            false,
            false,
        ));
    }

    #[test]
    fn escape_and_rating_keys_follow_edit_then_fullscreen_order() {
        assert!(rating_keys_apply(false, false));
        assert!(!rating_keys_apply(true, false));
        assert!(!rating_keys_apply(false, true));
        assert_eq!(
            escape_action(EscapeContext {
                context_menu_open: true,
                is_cropping: true,
                is_healing: true,
                empty_rating_filter: true,
                is_fullscreen: true,
            }),
            EscapeAction::CloseContextMenu
        );
        assert_eq!(
            escape_action(EscapeContext {
                context_menu_open: false,
                is_cropping: true,
                is_healing: true,
                empty_rating_filter: true,
                is_fullscreen: true,
            }),
            EscapeAction::CancelCrop
        );
        assert_eq!(
            escape_action(EscapeContext {
                context_menu_open: false,
                is_cropping: false,
                is_healing: true,
                empty_rating_filter: true,
                is_fullscreen: true,
            }),
            EscapeAction::LeaveHeal
        );
        assert_eq!(
            escape_action(EscapeContext {
                context_menu_open: false,
                is_cropping: false,
                is_healing: false,
                empty_rating_filter: true,
                is_fullscreen: true,
            }),
            EscapeAction::ClearRatingFilter
        );
        assert_eq!(
            escape_action(EscapeContext {
                context_menu_open: false,
                is_cropping: false,
                is_healing: false,
                empty_rating_filter: false,
                is_fullscreen: true,
            }),
            EscapeAction::LeaveFullscreen
        );
        assert_eq!(
            escape_action(EscapeContext {
                context_menu_open: false,
                is_cropping: false,
                is_healing: false,
                empty_rating_filter: false,
                is_fullscreen: false,
            }),
            EscapeAction::None
        );
    }

    #[test]
    fn rating_keys_reject_repeat_and_non_digit_shortcuts() {
        assert_eq!(
            rating_assignment_for_key("0", false),
            Some(RatingAssignment::Clear)
        );
        assert_eq!(
            rating_assignment_for_key("4", false),
            Some(RatingAssignment::Set(
                crate::ratings::Rating::new(4).expect("valid rating")
            ))
        );
        assert_eq!(rating_assignment_for_key("4", true), None);
        assert_eq!(rating_assignment_for_key("9", false), None);
        assert_eq!(rating_assignment_for_key("t", false), None);
    }

    #[test]
    fn space_release_only_unwinds_while_held() {
        assert!(is_space_key(&Key::Named(NamedKey::Space)));
        assert!(is_space_key(&Key::Character(" ".into())));
        assert!(!is_space_key(&Key::Named(NamedKey::Enter)));
        assert!(space_release_must_unwind(
            &Key::Named(NamedKey::Space),
            ElementState::Released,
            true,
        ));
        assert!(!space_release_must_unwind(
            &Key::Named(NamedKey::Space),
            ElementState::Pressed,
            true,
        ));
        assert!(!space_release_must_unwind(
            &Key::Named(NamedKey::Space),
            ElementState::Released,
            false,
        ));
        assert!(space_tap_fits(false, false));
        assert!(!space_tap_fits(true, false));
        assert!(!space_tap_fits(false, true));
    }
}
