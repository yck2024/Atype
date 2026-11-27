use std::borrow::Cow;

use handy_keys::{Hotkey, Key, Modifiers};
use tauri_plugin_global_shortcut::Shortcut;

use crate::settings::{AppSettings, KeyboardImplementation};

#[cfg(target_os = "macos")]
pub mod tap;

fn is_macos_f1(hotkey: Hotkey) -> bool {
    cfg!(target_os = "macos") && hotkey.key == Some(Key::F1)
}

pub(crate) fn without_synthetic_fn(mut hotkey: Hotkey) -> Hotkey {
    if is_macos_f1(hotkey) {
        hotkey.modifiers.remove(Modifiers::FN);
    }
    hotkey
}

pub fn parse(binding: &str) -> handy_keys::Result<Hotkey> {
    binding.parse().map(without_synthetic_fn)
}

pub fn canonical(binding: &str) -> Cow<'_, str> {
    match parse(binding) {
        Ok(hotkey) if is_macos_f1(hotkey) => Cow::Owned(hotkey.to_handy_string()),
        _ => Cow::Borrowed(binding),
    }
}

pub fn binding_is_taken(settings: &AppSettings, id: &str, binding: &str) -> bool {
    fn taken<T: PartialEq>(
        settings: &AppSettings,
        id: &str,
        binding: &str,
        parse: impl Fn(&str) -> Option<T>,
    ) -> bool {
        let candidate = parse(binding);
        settings.bindings.iter().any(|(other_id, other)| {
            other_id != id
                && (other.current_binding == binding
                    || candidate.as_ref().is_some_and(|candidate| {
                        parse(&other.current_binding).as_ref() == Some(candidate)
                    }))
        })
    }

    match settings.keyboard_implementation {
        KeyboardImplementation::HandyKeys => taken(settings, id, binding, |raw| parse(raw).ok()),
        KeyboardImplementation::Tauri => taken(settings, id, binding, |raw| {
            canonical(raw).parse::<Shortcut>().ok()
        }),
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn f1_drops_fn_when_recorded_and_preserves_other_modifiers() {
        assert_eq!(canonical("fn+f1"), "f1");
        assert_eq!(canonical("Shift_Right + Function + F1"), "shift_right+f1");
        assert_eq!(canonical("f1"), "f1");
    }

    #[test]
    fn other_bindings_are_untouched() {
        for binding in ["option+space", "fn", "fn+space", "fn+f2", "fn+f20", "f24"] {
            assert_eq!(canonical(binding), binding);
        }
    }

    #[test]
    fn stored_bindings_reserve_f1_even_when_not_registered() {
        let mut settings = crate::settings::get_default_settings();
        settings.post_process_enabled = false;
        settings
            .bindings
            .get_mut("transcribe")
            .unwrap()
            .current_binding = "option+space".into();
        for implementation in [
            KeyboardImplementation::HandyKeys,
            KeyboardImplementation::Tauri,
        ] {
            settings.keyboard_implementation = implementation;
            for (existing, candidate) in [("fn+f1", "f1"), ("f1", "function + F1")] {
                settings
                    .bindings
                    .get_mut("transcribe_with_post_process")
                    .unwrap()
                    .current_binding = existing.into();
                assert!(binding_is_taken(&settings, "transcribe", candidate));
                assert!(binding_is_taken(&settings, "cancel", candidate));
                assert!(!binding_is_taken(&settings, "transcribe", "shift+f1"));
            }
            assert!(!binding_is_taken(
                &settings,
                "transcribe_with_post_process",
                "f1"
            ));
        }
    }
}
