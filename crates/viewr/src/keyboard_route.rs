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

#[must_use]
pub(crate) fn is_space_key(key: &Key) -> bool {
    matches!(key, Key::Named(NamedKey::Space))
        || matches!(key, Key::Character(character) if character.as_str() == " ")
}

#[must_use]
pub(crate) fn space_release_must_unwind(key: &Key, state: ElementState, space_held: bool) -> bool {
    space_held && state == ElementState::Released && is_space_key(key)
}

#[must_use]
pub(crate) fn route_consumed_keyboard_key(key: &Key, is_cropping: bool, is_healing: bool) -> bool {
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
            | NamedKey::F5,
        ) => true,
        Key::Named(NamedKey::ArrowDown | NamedKey::ArrowUp) => is_cropping,
        Key::Named(NamedKey::Escape) => is_cropping || is_healing,
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
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("+".into()),
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("j".into()),
            false,
            false,
        ));
        for key in ["b", "B", "m", "M", "x", "X"] {
            assert!(
                !route_consumed_keyboard_key(&Key::Character(key.into()), false, false),
                "unused culling key {key} must not be intercepted"
            );
        }
        assert!(route_consumed_keyboard_key(
            &Key::Character("x".into()),
            true,
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
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::ArrowRight),
            true,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::Escape),
            true,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::Escape),
            false,
            true,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::ArrowRight),
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::F5),
            false,
            false,
        ));
        for key in [
            NamedKey::Home,
            NamedKey::End,
            NamedKey::PageUp,
            NamedKey::PageDown,
        ] {
            assert!(route_consumed_keyboard_key(&Key::Named(key), false, false,));
        }
        assert!(!route_consumed_keyboard_key(
            &Key::Named(NamedKey::ArrowDown),
            false,
            true,
        ));
        assert!(!route_consumed_keyboard_key(
            &Key::Named(NamedKey::Enter),
            true,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("[".into()),
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("]".into()),
            false,
            false,
        ));
        assert!(!route_consumed_keyboard_key(
            &Key::Named(NamedKey::Space),
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
    }
}
