//! Tauri global-shortcut implementation
//!
//! This module provides shortcut functionality using Tauri's built-in
//! global-shortcut plugin.

use log::{debug, error, warn};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::atype::hotkey;
use crate::settings::{self, ShortcutBinding};
use crate::transcription_coordinator::InputSource;

use super::handler::handle_shortcut_event;

/// Initialize shortcuts using Tauri's global-shortcut plugin
pub fn init_shortcuts(app: &AppHandle) {
    let default_bindings = settings::get_default_settings().bindings;
    let user_settings = settings::load_or_create_app_settings(app);

    // Register all default shortcuts, applying user customizations
    for (id, default_binding) in default_bindings {
        if id == "cancel" {
            continue; // Skip cancel shortcut, it will be registered dynamically
        }
        // Skip post-processing shortcut when the feature is disabled
        if id == "transcribe_with_post_process" && !user_settings.post_process_enabled {
            continue;
        }
        let binding = user_settings
            .bindings
            .get(&id)
            .cloned()
            .unwrap_or(default_binding);

        if let Err(e) = register_shortcut(app, binding) {
            error!("Failed to register shortcut {} during init: {}", id, e);
        }
    }
}

/// Validate a shortcut string for the Tauri global-shortcut implementation.
/// Tauri requires at least one non-modifier key and doesn't support the fn key.
pub fn validate_shortcut(raw: &str) -> Result<(), String> {
    parse_shortcut(raw).map(|_| ())
}

fn parse_shortcut(raw: &str) -> Result<Shortcut, String> {
    let raw = hotkey::for_tauri(raw);
    if raw.trim().is_empty() {
        return Err("Shortcut cannot be empty".into());
    }

    let modifiers = [
        "ctrl", "control", "shift", "alt", "option", "meta", "command", "cmd", "super", "win",
        "windows",
    ];

    // Check for fn key which Tauri doesn't support
    let parts: Vec<String> = raw.split('+').map(|p| p.trim().to_lowercase()).collect();
    for part in &parts {
        if part == "fn" || part == "function" {
            return Err("The 'fn' key is not supported by Tauri global shortcuts".into());
        }
    }

    // Check for at least one non-modifier key
    let has_non_modifier = parts.iter().any(|part| !modifiers.contains(&part.as_str()));

    if !has_non_modifier {
        return Err("Tauri shortcuts must include a main key (letter, number, F-key, etc.) in addition to modifiers".into());
    }

    // The name check above passes side-specific modifiers such as
    // `option_left`, which the handy-keys recorder saves but the accelerator
    // parser rejects. Parse here so a binding carried over from handy-keys is
    // reset to the default instead of failing to register.
    raw.parse::<Shortcut>()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", raw, e))
}

/// Register a shortcut using Tauri's global-shortcut plugin
pub fn register_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    register(app, binding, InputSource::Tauri)
}

#[cfg(target_os = "macos")]
pub fn register_shadow(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    register(app, binding, InputSource::Carbon)
}

fn register(app: &AppHandle, binding: ShortcutBinding, source: InputSource) -> Result<(), String> {
    let shortcut = parse_shortcut(&binding.current_binding)?;
    hotkey::update_tauri_registration(
        app,
        shortcut,
        (binding.id.clone(), source),
        true,
        move |app| register_parsed(app, binding, shortcut, source),
    )
}

fn register_parsed(
    app: &AppHandle,
    binding: ShortcutBinding,
    shortcut: Shortcut,
    source: InputSource,
) -> Result<(), String> {
    // Prevent duplicate registrations that would silently shadow one another
    if app.global_shortcut().is_registered(shortcut) {
        let error_msg = format!("Shortcut '{}' is already in use", binding.current_binding);
        warn!("register_tauri_shortcut duplicate error: {}", error_msg);
        return Err(error_msg);
    }

    // Clone binding.id for use in the closure
    let binding_id_for_closure = binding.id.clone();

    app.global_shortcut()
        .on_shortcut(shortcut, move |app_handle, scut, event| {
            if scut == &shortcut {
                let shortcut_string = scut.into_string();
                let is_pressed = event.state == ShortcutState::Pressed;
                // Mirrors the handy-keys event log line; the distinct prefix
                // makes it possible to tell which backend fired a shortcut
                // (e.g. when diagnosing the Secure Input fallback)
                debug!(
                    "tauri global-shortcut event: binding={}, shortcut={}, state={:?}",
                    binding_id_for_closure, shortcut_string, event.state
                );
                handle_shortcut_event(
                    app_handle,
                    &binding_id_for_closure,
                    &shortcut_string,
                    is_pressed,
                    source,
                );
            }
        })
        .map_err(|e| {
            let error_msg = format!(
                "Couldn't register shortcut '{}': {}",
                binding.current_binding, e
            );
            error!("register_tauri_shortcut registration error: {}", error_msg);
            error_msg
        })?;

    Ok(())
}

/// Unregister a shortcut from Tauri's global-shortcut plugin
pub fn unregister_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    unregister(app, binding, InputSource::Tauri)
}

#[cfg(target_os = "macos")]
pub fn unregister_shadow(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    unregister(app, binding, InputSource::Carbon)
}

fn unregister(
    app: &AppHandle,
    binding: ShortcutBinding,
    source: InputSource,
) -> Result<(), String> {
    let shortcut = parse_shortcut(&binding.current_binding)?;
    hotkey::update_tauri_registration(app, shortcut, (binding.id, source), false, move |app| {
        app.global_shortcut().unregister(shortcut).map_err(|e| {
            let error_msg = format!(
                "Failed to unregister shortcut '{}': {}",
                binding.current_binding, e
            );
            error!("unregister_tauri_shortcut error: {}", error_msg);
            error_msg
        })
    })
}

#[cfg(test)]
mod tests {
    use super::validate_shortcut;

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_legacy_f1_as_the_same_accelerator_for_validation_and_registration() {
        use super::parse_shortcut;
        assert_eq!(validate_shortcut("fn+f1"), Ok(()));
        assert_eq!(parse_shortcut("fn+shift+f1"), parse_shortcut("shift+f1"));
        assert_eq!(parse_shortcut("Function + F1"), parse_shortcut("f1"));
        for invalid in [
            "fn+f2",
            "f1+shift",
            "option++f1",
            "fn+f1+shift",
            "option++fn+f1",
            "option_left+fn+f1",
            "win+fn+f1",
        ] {
            assert!(
                validate_shortcut(invalid).is_err(),
                "{invalid} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_side_specific_modifiers_the_parser_cannot_register() {
        for raw in ["option_left+space", "ctrl_right+space"] {
            assert!(validate_shortcut(raw).is_err(), "{raw} should be rejected");
        }
    }

    #[test]
    fn accepts_the_default_shortcuts() {
        for raw in [
            "option+space",
            "option+shift+space",
            "ctrl+space",
            "ctrl+shift+space",
            "alt+space",
            "escape",
        ] {
            assert_eq!(validate_shortcut(raw), Ok(()), "{raw} should be accepted");
        }
    }
}
