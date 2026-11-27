use std::borrow::Cow;

use handy_keys::{Hotkey, Key, Modifiers};

use crate::settings::AppSettings;

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

pub fn for_tap(binding: &str) -> handy_keys::Result<Hotkey> {
    let mut hotkey = binding.parse()?;
    if is_macos_f1(hotkey) {
        hotkey.modifiers.insert(Modifiers::FN);
    }
    Ok(hotkey)
}

#[cfg(target_os = "macos")]
pub fn is_f1_binding(binding: &str) -> bool {
    binding.parse().is_ok_and(is_macos_f1)
}

pub fn f1_conflict(settings: &AppSettings, id: &str, binding: &str) -> bool {
    let Ok(candidate) = parse(binding) else {
        return false;
    };
    is_macos_f1(candidate)
        && settings.bindings.iter().any(|(other_id, other)| {
            other_id != id && parse(&other.current_binding).is_ok_and(|key| key == candidate)
        })
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
    fn f1_alone_gets_fn_for_the_existing_tap() {
        assert_eq!(for_tap("f1").unwrap(), "fn+f1".parse().unwrap());
        assert_eq!(
            for_tap("fn+shift+f1").unwrap(),
            for_tap("shift+f1").unwrap()
        );
        for binding in [
            "option+space",
            "fn",
            "fn+space",
            "f2",
            "fn+f2",
            "fn+f20",
            "f24",
        ] {
            assert_eq!(canonical(binding), binding);
            assert_eq!(for_tap(binding).ok(), binding.parse::<Hotkey>().ok());
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
        for (existing, candidate) in [
            ("fn+f1", "f1"),
            ("f1", "function + F1"),
            ("fn+shift+f1", "shift+f1"),
        ] {
            settings
                .bindings
                .get_mut("transcribe_with_post_process")
                .unwrap()
                .current_binding = existing.into();
            assert!(f1_conflict(&settings, "transcribe", candidate));
            assert!(f1_conflict(&settings, "cancel", candidate));
            assert!(!f1_conflict(
                &settings,
                "transcribe_with_post_process",
                candidate
            ));
            assert!(!f1_conflict(&settings, "transcribe", "command+f1"));
        }
        assert!(!f1_conflict(&settings, "transcribe", "escape"));
    }
}
