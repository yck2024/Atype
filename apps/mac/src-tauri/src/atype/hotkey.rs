//! macOS sends a synthetic Fn flag with F1 events even in standard function-key
//! mode. The event tap needs that flag to match; bindings and Carbon accelerators
//! treat it as part of F1, not as a separate modifier.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{mpsc, Mutex};

use handy_keys::{Hotkey, Key, Modifiers};
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, Shortcut};

use crate::settings::AppSettings;
use crate::transcription_coordinator::InputSource;

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

pub fn for_tauri(binding: &str) -> Cow<'_, str> {
    let tokens = binding.split('+');
    let is_fn = |token: &str| {
        let token = token.trim();
        token.eq_ignore_ascii_case("fn") || token.eq_ignore_ascii_case("function")
    };
    if cfg!(target_os = "macos")
        && tokens
            .clone()
            .any(|token| token.trim().eq_ignore_ascii_case("f1"))
        && tokens.clone().any(is_fn)
    {
        Cow::Owned(
            tokens
                .filter(|token| !is_fn(token))
                .collect::<Vec<_>>()
                .join("+")
                .trim()
                .to_owned(),
        )
    } else {
        Cow::Borrowed(binding)
    }
}

pub fn is_f1_binding(binding: &str) -> bool {
    cfg!(target_os = "macos") && binding.parse().is_ok_and(is_macos_f1)
}

type Owner = (String, InputSource);

#[derive(Default)]
struct F1Owners(HashMap<u32, Owner>);

impl F1Owners {
    fn update(
        &mut self,
        shortcut: Shortcut,
        owner: Owner,
        registering: bool,
        operation: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if !registering && self.0.get(&shortcut.id()) != Some(&owner) {
            return Ok(());
        }
        operation()?;
        if registering {
            self.0.insert(shortcut.id(), owner);
        } else {
            self.0.remove(&shortcut.id());
        }
        Ok(())
    }
}

pub fn update_tauri_registration(
    app: &AppHandle,
    shortcut: Shortcut,
    owner: Owner,
    registering: bool,
    operation: impl FnOnce(&AppHandle) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    if !cfg!(target_os = "macos") || shortcut.key != Code::F1 {
        return operation(app);
    }
    let (tx, rx) = mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        if handle.try_state::<Mutex<F1Owners>>().is_none() {
            handle.manage(Mutex::new(F1Owners::default()));
        }
        let result = handle.state::<Mutex<F1Owners>>().lock().unwrap().update(
            shortcut,
            owner,
            registering,
            || operation(&handle),
        );
        let _ = tx.send(result);
    })
    .map_err(|err| err.to_string())?;
    rx.recv().map_err(|err| err.to_string())?
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
    fn f1_unregistration_requires_the_successful_binding_and_backend() {
        let mut owners = F1Owners::default();
        let shortcut: Shortcut = "f1".parse().unwrap();
        let alias: Shortcut = for_tauri("fn+f1").parse().unwrap();
        let main = ("transcribe".into(), InputSource::Tauri);
        let ai = ("transcribe_with_post_process".into(), InputSource::Tauri);
        let shadow = ("transcribe".into(), InputSource::Carbon);
        owners
            .update(shortcut, main.clone(), true, || Ok(()))
            .unwrap();
        assert!(owners
            .update(alias, ai.clone(), true, || Err("already registered".into()))
            .is_err());
        for other in [ai, shadow] {
            owners
                .update(alias, other, false, || {
                    panic!("non-owner tried to unregister F1")
                })
                .unwrap();
        }
        assert!(owners
            .update(alias, main.clone(), false, || Err("native error".into()))
            .is_err());
        let mut removed = false;
        owners
            .update(alias, main.clone(), false, || {
                removed = true;
                Ok(())
            })
            .unwrap();
        assert!(removed);
        owners
            .update(shortcut, main, false, || {
                panic!("registration was already removed")
            })
            .unwrap();
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
