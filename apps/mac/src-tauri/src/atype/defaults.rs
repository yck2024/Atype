//! One-time Atype defaults for an existing settings store.
//!
//! Handy writes its full settings on first launch, so changing a `default_*`
//! function only affects brand-new installs. This applies Atype's choices once
//! to a store that already exists, then records the version in `atype.json`
//! so later changes the user makes in the UI are never overwritten.

use super::config;
use crate::settings::{
    get_settings, write_settings, AppSettings, ChineseScript, DEFAULT_GEMINI_MODEL,
    DEFAULT_PROMPT_ID,
};
use log::info;
use tauri::AppHandle;

pub const VERSION: u32 = 6;

/// Version 1: LLM cleanup on, Traditional output, a useful history size,
/// no upstream "what's new" notes.
pub fn apply_v1(s: &mut AppSettings) {
    s.post_process_enabled = true;
    s.chinese_script = ChineseScript::Traditional;
    s.history_limit = s.history_limit.max(300);
    s.show_whats_new_on_update = false;
}

/// Version 2: launch quietly into the menu bar (no window, no Dock icon).
/// The Dock icon appears only while the settings window is open. Only applied
/// once setup is done (a model is selected); macOS does not force the window
/// open for onboarding, so a fresh install must still start with the window.
pub fn apply_v2(s: &mut AppSettings) {
    s.start_hidden = true;
    s.show_tray_icon = true;
}

/// Version 3: make the LLM actually run, and add the format presets
/// (email, chat reply, meeting notes, ...). Select 萬用口令 (the command
/// hotkey's prompt) when none is selected (Handy shipped with none, so every call was
/// skipped), and replace an empty or unusable Gemini model (live, TTS, ...)
/// with the default.
pub fn apply_v3(s: &mut AppSettings) {
    super::prompts::add_missing(&mut s.post_process_prompts);
    let selected_exists = s
        .post_process_selected_prompt_id
        .as_ref()
        .is_some_and(|id| s.post_process_prompts.iter().any(|p| &p.id == id));
    if !selected_exists {
        if let Some(prompt) = s
            .post_process_prompts
            .iter()
            .find(|p| p.id == DEFAULT_PROMPT_ID)
            .or_else(|| s.post_process_prompts.first())
        {
            s.post_process_selected_prompt_id = Some(prompt.id.clone());
        }
    }
    let gemini = s
        .post_process_models
        .entry("gemini".to_string())
        .or_default();
    if gemini.trim().is_empty() || !super::is_chat_model(gemini) {
        *gemini = DEFAULT_GEMINI_MODEL.to_string();
    }
}

/// Version 4: refresh unedited preset prompts (keep Chinese names in Chinese).
pub fn apply_v4(s: &mut AppSettings) {
    super::prompts::refresh_unedited(&mut s.post_process_prompts);
}

/// Version 5: refresh unedited presets again (萬用口令: 正式 is not a letter).
pub fn apply_v5(s: &mut AppSettings) {
    super::prompts::refresh_unedited(&mut s.post_process_prompts);
}

/// The main hotkey's default before v6. Option+Space collides with other apps
/// (Chrome in particular).
pub const OLD_TRANSCRIBE_DEFAULT: &str = "option+space";

fn apply_v6(s: &mut AppSettings) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let new_default = crate::settings::get_default_settings().bindings["transcribe"]
        .default_binding
        .clone();
    let taken = super::hotkey::f1_conflict(s, "transcribe", &new_default);
    let Some(transcribe) = s.bindings.get_mut("transcribe") else {
        return false;
    };
    if transcribe.default_binding == new_default {
        return false;
    }
    if !taken && transcribe.current_binding == OLD_TRANSCRIBE_DEFAULT {
        transcribe.current_binding = new_default.clone();
    }
    transcribe.default_binding = new_default;
    true
}

/// Setup is done once a transcription model has been chosen.
pub fn setup_done(s: &AppSettings) -> bool {
    !s.selected_model.trim().is_empty()
}

pub fn apply_once(app: &AppHandle) {
    let mut cfg = config::load(app);
    let mut settings = get_settings(app);
    let (applied, migrated_f1) = apply_pending(&mut settings, cfg.defaults_version);
    if migrated_f1 || applied != cfg.defaults_version {
        write_settings(app, settings);
    }
    if applied != cfg.defaults_version {
        cfg.defaults_version = applied;
        config::save(app, &cfg);
        info!("Atype: applied one-time defaults up to v{}", applied);
    }
}

fn apply_pending(settings: &mut AppSettings, mut applied: u32) -> (u32, bool) {
    let migrated_f1 = apply_v6(settings);
    if applied < 1 {
        apply_v1(settings);
        applied = 1;
    }
    if applied < 2 && setup_done(settings) {
        apply_v2(settings);
        applied = 2;
    }
    if applied == 2 {
        apply_v3(settings);
        applied = 3;
    }
    if applied == 3 {
        apply_v4(settings);
        applied = 4;
    }
    if applied == 4 {
        apply_v5(settings);
        applied = 5;
    }
    if applied == 5 {
        applied = VERSION;
    }
    (applied, migrated_f1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_sets_atype_choices_and_never_shrinks_history() {
        let mut s = crate::settings::get_default_settings();
        s.post_process_enabled = false;
        s.chinese_script = ChineseScript::AsTranscribed;
        s.history_limit = 5;
        s.show_whats_new_on_update = true;
        apply_v1(&mut s);
        assert!(s.post_process_enabled);
        assert_eq!(s.chinese_script, ChineseScript::Traditional);
        assert_eq!(s.history_limit, 300);
        assert!(!s.show_whats_new_on_update);

        s.history_limit = 1000;
        apply_v1(&mut s);
        assert_eq!(s.history_limit, 1000);
    }

    #[test]
    fn v2_waits_for_setup() {
        let mut s = crate::settings::get_default_settings();
        s.selected_model = String::new();
        assert!(!setup_done(&s));
        s.selected_model = "handy-computer/SenseVoiceSmall-gguf/x.gguf".into();
        assert!(setup_done(&s));
    }

    #[test]
    fn v2_starts_in_the_menu_bar() {
        let mut s = crate::settings::get_default_settings();
        s.start_hidden = false;
        s.show_tray_icon = false;
        apply_v2(&mut s);
        assert!(s.start_hidden);
        assert!(s.show_tray_icon);
    }

    #[test]
    fn v3_selects_the_prompt_and_fixes_the_gemini_model() {
        let mut s = crate::settings::get_default_settings();
        s.post_process_selected_prompt_id = None;
        s.post_process_models
            .insert("gemini".into(), "models/gemini-3.8-live".into());
        apply_v3(&mut s);
        assert_eq!(
            s.post_process_selected_prompt_id.as_deref(),
            Some(DEFAULT_PROMPT_ID)
        );
        assert_eq!(s.post_process_models["gemini"], DEFAULT_GEMINI_MODEL);
    }

    fn set_transcribe(s: &mut AppSettings, current: &str, default: &str) {
        let b = s.bindings.get_mut("transcribe").unwrap();
        b.current_binding = current.into();
        b.default_binding = default.into();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn v6_moves_the_old_default_to_f1() {
        let mut s = crate::settings::get_default_settings();
        set_transcribe(&mut s, OLD_TRANSCRIBE_DEFAULT, OLD_TRANSCRIBE_DEFAULT);
        apply_v6(&mut s);
        assert_eq!(s.bindings["transcribe"].current_binding, "f1");
        assert_eq!(s.bindings["transcribe"].default_binding, "f1");
        // The other shortcuts are untouched.
        let defaults = crate::settings::get_default_settings();
        for id in ["transcribe_with_post_process", "cancel"] {
            assert_eq!(
                s.bindings[id].current_binding,
                defaults.bindings[id].current_binding
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn v6_keeps_a_binding_the_user_picked() {
        let mut s = crate::settings::get_default_settings();
        set_transcribe(&mut s, "option_right+command_right", OLD_TRANSCRIBE_DEFAULT);
        apply_v6(&mut s);
        assert_eq!(
            s.bindings["transcribe"].current_binding,
            "option_right+command_right"
        );
        // Reset now goes to F1.
        assert_eq!(s.bindings["transcribe"].default_binding, "f1");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn v6_leaves_the_hotkey_alone_when_f1_is_taken() {
        let mut s = crate::settings::get_default_settings();
        set_transcribe(&mut s, OLD_TRANSCRIBE_DEFAULT, OLD_TRANSCRIBE_DEFAULT);
        s.bindings
            .get_mut("transcribe_with_post_process")
            .unwrap()
            .current_binding = "fn+f1".into();
        apply_v6(&mut s);
        assert_eq!(
            s.bindings["transcribe"].current_binding,
            OLD_TRANSCRIBE_DEFAULT
        );
        assert_eq!(s.bindings["transcribe"].default_binding, "f1");
        assert_eq!(
            s.bindings["transcribe_with_post_process"].current_binding,
            "fn+f1"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn f1_migrates_at_launch_without_a_model_and_only_once() {
        let mut s = crate::settings::get_default_settings();
        s.selected_model.clear();
        s.start_hidden = false;
        set_transcribe(&mut s, OLD_TRANSCRIBE_DEFAULT, OLD_TRANSCRIBE_DEFAULT);
        assert_eq!(apply_pending(&mut s, 1), (1, true));
        assert!(!s.start_hidden);
        assert_eq!(s.bindings["transcribe"].current_binding, "f1");
        assert_eq!(s.bindings["transcribe"].default_binding, "f1");
        set_transcribe(&mut s, OLD_TRANSCRIBE_DEFAULT, "f1");
        s.selected_model = "handy-computer/SenseVoiceSmall-gguf/x.gguf".into();
        assert_eq!(apply_pending(&mut s, 1), (VERSION, false));
        assert!(s.start_hidden);
        assert_eq!(
            s.bindings["transcribe"].current_binding,
            OLD_TRANSCRIBE_DEFAULT
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn f1_migration_preserves_both_preexisting_aliases() {
        let mut s = crate::settings::get_default_settings();
        set_transcribe(&mut s, "fn+f1", OLD_TRANSCRIBE_DEFAULT);
        s.bindings
            .get_mut("transcribe_with_post_process")
            .unwrap()
            .current_binding = "f1".into();
        assert_eq!(apply_pending(&mut s, VERSION), (VERSION, true));
        assert_eq!(s.bindings["transcribe"].current_binding, "fn+f1");
        assert_eq!(s.bindings["transcribe"].default_binding, "f1");
        assert_eq!(
            s.bindings["transcribe_with_post_process"].current_binding,
            "f1"
        );
    }

    #[test]
    fn v3_keeps_a_working_choice() {
        let mut s = crate::settings::get_default_settings();
        s.post_process_models
            .insert("gemini".into(), "models/gemini-3.5-flash".into());
        apply_v3(&mut s);
        assert_eq!(s.post_process_models["gemini"], "models/gemini-3.5-flash");
    }
}
