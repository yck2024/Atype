//! macOS Secure Event Input detection, monitoring, and fallback.
//!
//! When any process enables secure event input (password fields, Terminal's
//! "Secure Keyboard Entry", a stuck `loginwindow`), CGEventTaps stop receiving
//! KeyDown/KeyUp events while FlagsChanged still flows. The handy-keys
//! implementation is tap-based, so keyed shortcuts (e.g. Option+Space) die
//! silently while modifier-only shortcuts keep working. See issue #1578.
//!
//! This module:
//! - polls `IsSecureEventInputEnabled()` and tracks state transitions
//! - looks up the holding process (best effort — Apple documents no reliable
//!   API; the IORegistry PID is frequently wrong or absent)
//! - as soon as secure input is observed, shadow-registers vulnerable *keyed*
//!   bindings through the Carbon-backed Tauri global-shortcut path, which is
//!   not affected by secure input (modifier-only bindings need no fallback).
//!   The user-facing warning still waits until secure input is *sustained*,
//!   so a momentary password-field focus never shows a banner, but dictation
//!   no longer has a multi-second dead window at the start of every episode
//! - drops those shadows while the settings window records a shortcut, so a
//!   Carbon hotkey cannot fire or swallow the keys being recorded
//! - dynamically shadows the Cancel binding while recording, so Escape and
//!   other keyed cancellation shortcuts remain available under secure input
//! - exposes a count-only keyboard diagnostic for the debug window. Only
//!   event *kinds* are counted — key identity is never logged or returned.

use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Serialize, Type)]
pub struct SecureInputStatus {
    /// Secure input is currently enabled (live check)
    pub enabled: bool,
    /// Enabled continuously long enough to be considered stuck (not just a
    /// password field gaining momentary focus)
    pub sustained: bool,
    pub culprit_pid: Option<i32>,
    pub culprit_name: Option<String>,
    /// Carbon fallback registrations are currently active
    pub fallback_active: bool,
    /// Binding ids shadow-registered with identical semantics
    pub covered_bindings: Vec<String>,
    /// Side-specific binding ids widened to match either side while shadowed
    pub degraded_bindings: Vec<String>,
    /// Binding ids that cannot fire at all (e.g. fn+key, registration failure)
    pub uncovered_bindings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct KeyboardDiagnosticReport {
    pub secure_input_enabled: bool,
    pub culprit_pid: Option<i32>,
    pub culprit_name: Option<String>,
    /// Counts only — key identity is deliberately never captured.
    pub key_down: u32,
    pub key_up: u32,
    pub flags_changed: u32,
    pub mouse: u32,
    pub duration_ms: u32,
}

#[tauri::command]
#[specta::specta]
pub fn get_secure_input_status(app: AppHandle) -> SecureInputStatus {
    imp::status(&app)
}

#[tauri::command]
#[specta::specta]
pub async fn run_keyboard_diagnostic(
    duration_secs: Option<u32>,
) -> Result<KeyboardDiagnosticReport, String> {
    imp::run_diagnostic(duration_secs.unwrap_or(10).clamp(3, 30)).await
}

/// True if secure input is enabled right now (live check, macOS only).
pub fn is_enabled_now() -> bool {
    imp::is_enabled()
}

/// Suspend or resume the Carbon fallback shadows while the settings window
/// records a shortcut. Mirrors `shortcut::suspend_all_shortcuts`: a Carbon
/// hotkey would otherwise fire (and consume) the very keys being recorded,
/// which matters when the recorder reads the window's own key events because
/// another app holds Secure Input.
pub fn set_shortcuts_suspended(app: &AppHandle, suspended: bool) {
    imp::set_shortcuts_suspended(app, suspended)
}

/// Register/unregister the dynamic Cancel binding through the Carbon fallback
/// while a recording and sustained Secure Input overlap.
pub fn register_cancel_fallback(app: &AppHandle) {
    imp::register_cancel_fallback(app)
}

pub fn unregister_cancel_fallback(app: &AppHandle) {
    imp::unregister_cancel_fallback(app)
}

/// Synchronize Carbon fallback registrations with current settings and
/// lifecycle state while preserving unchanged registrations.
pub fn reconcile_fallback(app: &AppHandle) {
    imp::reconcile_fallback(app)
}

/// Managed state + monitor startup. On non-macOS platforms the state exists
/// but the monitor never runs and everything reports disabled.
pub fn init(app: &AppHandle) {
    app.manage(imp::SecureInputState::new());
    imp::start_monitor(app);
}

/// Whether the tray should show the warning badge / menu entry.
///
/// Only when the user is actually impacted: secure input is sustained and a
/// binding is degraded or dead.
/// When every affected binding is covered transparently by the fallback (or
/// none are affected), the experience is seamless and nothing is shown.
pub fn tray_warning_active(app: &AppHandle) -> bool {
    app.try_state::<imp::SecureInputState>()
        .map(|s| s.warning_active())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use crate::settings::{self, KeyboardImplementation, ShortcutBinding};
    use log::{debug, error, info, warn};
    use std::collections::HashMap;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// How often the monitor thread polls.
    const POLL_INTERVAL: Duration = Duration::from_secs(1);
    /// Secure input must be held this long before we *warn* about it.
    /// Momentary activation (a password field gaining focus) is normal. The
    /// Carbon fallback does not wait for this: it is armed on the first
    /// enabled sample so keyed dictation shortcuts keep working throughout.
    const SUSTAIN_THRESHOLD: Duration = Duration::from_secs(3);

    /// Transitions reported by [`EpisodeTracker::observe`] for one sample.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub(super) struct EpisodeStep {
        /// Secure input was just observed enabled: arm the Carbon fallback.
        pub became_enabled: bool,
        /// Held past [`SUSTAIN_THRESHOLD`]: the warning may now surface.
        pub became_sustained: bool,
        /// Released: drop the fallback and clear every warning.
        pub became_disabled: bool,
    }

    /// Pure Secure Input episode state machine, fed one sample per poll.
    /// Kept free of `AppHandle` so tests can drive it with injected samples
    /// and timestamps.
    #[derive(Debug, Default)]
    pub(super) struct EpisodeTracker {
        enabled_since: Option<Instant>,
        sustained: bool,
    }

    impl EpisodeTracker {
        pub fn observe(&mut self, enabled: bool, now: Instant) -> EpisodeStep {
            let mut step = EpisodeStep::default();
            match (enabled, self.enabled_since) {
                (true, None) => {
                    self.enabled_since = Some(now);
                    step.became_enabled = true;
                }
                (true, Some(since)) => {
                    if !self.sustained && now.saturating_duration_since(since) >= SUSTAIN_THRESHOLD
                    {
                        self.sustained = true;
                        step.became_sustained = true;
                    }
                }
                (false, Some(_)) => {
                    self.enabled_since = None;
                    self.sustained = false;
                    step.became_disabled = true;
                }
                (false, None) => {}
            }
            step
        }
    }

    /// Inputs that decide which bindings the Carbon fallback should shadow.
    #[derive(Debug, Clone, Copy)]
    pub(super) struct FallbackContext {
        /// Secure input observed, shortcuts initialized, handy-keys active.
        pub armed: bool,
        /// The settings window is recording a shortcut.
        pub suspended: bool,
        /// An audio recording wants the dynamic Cancel binding.
        pub cancel_requested: bool,
        pub post_process_enabled: bool,
    }

    /// Binding ids the fallback should consider, in a stable order.
    pub(super) fn fallback_candidates(
        bindings: &HashMap<String, ShortcutBinding>,
        ctx: FallbackContext,
    ) -> Vec<String> {
        if !ctx.armed {
            return Vec::new();
        }
        let mut ids: Vec<String> = bindings
            .keys()
            .filter(|id| {
                if id.as_str() == "cancel" {
                    // Managed by the audio-recording lifecycle, like the
                    // primary implementation; recording a shortcut in the
                    // settings window leaves it untouched.
                    return ctx.cancel_requested;
                }
                if ctx.suspended {
                    return false;
                }
                id.as_str() != "transcribe_with_post_process" || ctx.post_process_enabled
            })
            .cloned()
            .collect();
        ids.sort();
        ids
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        // Carbon HIToolbox; Boolean is an unsigned char
        fn IsSecureEventInputEnabled() -> u8;
    }

    pub fn is_enabled() -> bool {
        unsafe { IsSecureEventInputEnabled() != 0 }
    }

    #[derive(Debug, Clone)]
    struct Culprit {
        pid: i32,
        name: String,
    }

    #[derive(Default)]
    struct FallbackState {
        /// Bindings shadow-registered through the Tauri/Carbon path (possibly
        /// with widened modifiers), kept so deactivation unregisters the
        /// exact strings we registered.
        registered: Vec<ShortcutBinding>,
        /// Shadowed with identical semantics
        covered: Vec<String>,
        /// Shadowed, but side-specific modifiers widened to either side
        degraded: Vec<String>,
        /// Cannot fire at all while secure input is held
        uncovered: Vec<String>,
    }

    pub struct SecureInputState {
        /// Secure input was enabled at the monitor's last sample. Arms the
        /// Carbon fallback.
        enabled: AtomicBool,
        /// Held past the sustain threshold. Gates user-facing warnings only.
        sustained: AtomicBool,
        culprit: Mutex<Option<Culprit>>,
        fallback: Mutex<FallbackState>,
        /// Serializes fallback registration changes without requiring the
        /// fallback state lock to be held across global-shortcut plugin calls.
        fallback_operation: Mutex<()>,
        cancel_requested: AtomicBool,
        /// The settings window is recording a shortcut; keep shadows off.
        shortcuts_suspended: AtomicBool,
        monitor_started: AtomicBool,
    }

    impl SecureInputState {
        pub fn new() -> Self {
            Self {
                enabled: AtomicBool::new(false),
                sustained: AtomicBool::new(false),
                culprit: Mutex::new(None),
                fallback: Mutex::new(FallbackState::default()),
                fallback_operation: Mutex::new(()),
                cancel_requested: AtomicBool::new(false),
                shortcuts_suspended: AtomicBool::new(false),
                monitor_started: AtomicBool::new(false),
            }
        }

        pub fn is_sustained(&self) -> bool {
            self.sustained.load(Ordering::SeqCst)
        }

        fn is_observed_enabled(&self) -> bool {
            self.enabled.load(Ordering::SeqCst)
        }

        /// User-visible impact exists: secure input is sustained and some
        /// binding is degraded or dead. Released secure input never warns.
        pub fn warning_active(&self) -> bool {
            if !self.is_sustained() {
                return false;
            }
            let fallback = self.fallback.lock().unwrap();
            !fallback.degraded.is_empty() || !fallback.uncovered.is_empty()
        }
    }

    /// Best-effort culprit lookup via the IORegistry session property.
    /// Apple documents no reliable API for this; the PID may be missing
    /// (an app quit while holding secure input) or point at the wrong
    /// process (often the responsible parent, or `loginwindow`).
    fn lookup_culprit() -> Option<Culprit> {
        let out = Command::new("ioreg")
            .args(["-l", "-w", "0"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let pid: i32 = text
            .lines()
            .find_map(|l| l.split("\"kCGSSessionSecureInputPID\"=").nth(1))?
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()?;

        // `ps -o comm=` returns the full executable path; show just the
        // binary name ("Terminal", not ".../Terminal.app/Contents/MacOS/Terminal")
        let name = Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .output()
            .ok()
            .and_then(|o| {
                let raw = String::from_utf8_lossy(&o.stdout);
                let trimmed = raw.trim();
                (!trimmed.is_empty())
                    .then(|| trimmed.rsplit('/').next().unwrap_or(trimmed).to_string())
            })
            .unwrap_or_else(|| "(process no longer running)".to_string());

        Some(Culprit { pid, name })
    }

    pub fn status(app: &AppHandle) -> SecureInputStatus {
        let enabled = is_enabled();
        let state = app.state::<SecureInputState>();

        // Culprit discovery shells out to ioreg and is intentionally performed
        // only by the monitor (or the blocking diagnostic), never by this
        // synchronous Tauri command.
        let culprit = state.culprit.lock().unwrap().clone();
        let fallback = state.fallback.lock().unwrap();
        SecureInputStatus {
            enabled,
            sustained: state.sustained.load(Ordering::SeqCst),
            culprit_pid: culprit.as_ref().map(|c| c.pid),
            culprit_name: culprit.map(|c| c.name),
            fallback_active: !fallback.registered.is_empty(),
            covered_bindings: fallback.covered.clone(),
            degraded_bindings: fallback.degraded.clone(),
            uncovered_bindings: fallback.uncovered.clone(),
        }
    }

    pub fn set_shortcuts_suspended(app: &AppHandle, suspended: bool) {
        let state = app.state::<SecureInputState>();
        let was = state.shortcuts_suspended.swap(suspended, Ordering::SeqCst);
        // Only an armed fallback has shadows to drop or restore.
        if was != suspended && state.is_observed_enabled() {
            reconcile_fallback(app);
        }
    }

    fn emit_status(app: &AppHandle) {
        let payload = status(app);
        if let Err(e) = app.emit("secure-input-changed", &payload) {
            error!("Failed to emit secure-input-changed: {e}");
        }
    }

    fn refresh_tray(app: &AppHandle) {
        // No-op before the tray is built; otherwise a diffed, coalesced,
        // main-thread apply that never blocks this thread.
        crate::tray::refresh_tray_icon(app);
    }

    pub fn start_monitor(app: &AppHandle) {
        let state = app.state::<SecureInputState>();
        if state.monitor_started.swap(true, Ordering::SeqCst) {
            return;
        }

        let app = app.clone();
        std::thread::spawn(move || {
            info!("secure-input monitor started");
            let mut tracker = EpisodeTracker::default();
            loop {
                std::thread::sleep(POLL_INTERVAL);
                let state = app.state::<SecureInputState>();
                let step = tracker.observe(is_enabled(), Instant::now());

                if step.became_enabled {
                    let culprit = lookup_culprit();
                    match &culprit {
                        Some(c) => {
                            info!("SecureInput ENABLED (held by pid {} '{}')", c.pid, c.name)
                        }
                        None => info!("SecureInput ENABLED (no visible holder)"),
                    }
                    *state.culprit.lock().unwrap() = culprit;
                    state.enabled.store(true, Ordering::SeqCst);
                    // Arm the fallback right away: the event tap is already
                    // blind to KeyDown, so waiting would leave keyed
                    // dictation shortcuts dead for the threshold.
                    reconcile_fallback(&app);
                }

                if step.became_sustained {
                    warn!(
                        "SecureInput held for {}s — keyed shortcuts rely on the Carbon fallback",
                        SUSTAIN_THRESHOLD.as_secs()
                    );
                    state.sustained.store(true, Ordering::SeqCst);
                    // Retries any shadow that failed to register and
                    // publishes the (now user-visible) warning state.
                    reconcile_fallback(&app);
                }

                if step.became_disabled {
                    info!("SecureInput DISABLED");
                    *state.culprit.lock().unwrap() = None;
                    state.enabled.store(false, Ordering::SeqCst);
                    state.sustained.store(false, Ordering::SeqCst);
                    // Drops every shadow and emits the cleared status, so the
                    // banner and tray badge go away on release.
                    reconcile_fallback(&app);
                }
            }
        });
    }

    fn is_mouse_key(key: &handy_keys::Key) -> bool {
        key.to_string().to_lowercase().starts_with("mouse")
    }

    /// Build the Carbon-registrable equivalent of a keyed hotkey.
    ///
    /// Carbon has no concept of left/right modifiers, so side-specific
    /// modifiers widen to the whole group — returned as `degraded: true` so
    /// the UI can call out the changed matching. The fn key cannot be
    /// expressed at all (`None`).
    fn carbon_equivalent(hotkey: &handy_keys::Hotkey) -> Option<(String, bool)> {
        use handy_keys::Modifiers as M;

        if hotkey.modifiers.contains(M::FN) {
            return None;
        }

        let mut widened = M::empty();
        let mut degraded = false;
        for group in [M::CTRL, M::OPT, M::SHIFT, M::CMD] {
            if hotkey.modifiers.intersects(group) {
                widened |= group;
                if !hotkey.modifiers.contains(group) {
                    // Only one side was specified — matching gets wider
                    degraded = true;
                }
            }
        }

        let carbon_hotkey = handy_keys::Hotkey::new(widened, hotkey.key).ok()?;
        Some((carbon_hotkey.to_handy_string(), degraded))
    }

    /// Desired fallback for one binding, computed without plugin calls.
    pub(super) enum ShadowPlan {
        /// Modifier-only or mouse-based; unaffected by secure input.
        Immune,
        /// Cannot be represented through Carbon.
        Uncovered,
        /// Register this shadow through Carbon.
        Shadow {
            shadow: ShortcutBinding,
            degraded: bool,
        },
    }

    pub(super) fn plan_fallback_binding(id: &str, binding: &ShortcutBinding) -> ShadowPlan {
        let Ok(hotkey) = binding.current_binding.parse::<handy_keys::Hotkey>() else {
            warn!(
                "SecureInput fallback: '{}' has unparseable binding '{}', skipping",
                id, binding.current_binding
            );
            return ShadowPlan::Uncovered;
        };

        match &hotkey.key {
            None => {
                debug!(
                    "SecureInput fallback: '{}' ('{}') is modifier-only — immune, no shadow needed",
                    id, binding.current_binding
                );
                return ShadowPlan::Immune;
            }
            Some(k) if is_mouse_key(k) => {
                debug!(
                    "SecureInput fallback: '{}' ('{}') is mouse-based — immune, no shadow needed",
                    id, binding.current_binding
                );
                return ShadowPlan::Immune;
            }
            Some(_) => {}
        }

        let Some((carbon_binding, degraded)) = carbon_equivalent(&hotkey) else {
            warn!(
                "SecureInput fallback: '{}' ('{}') cannot be expressed via Carbon",
                id, binding.current_binding
            );
            return ShadowPlan::Uncovered;
        };

        let mut shadow = binding.clone();
        shadow.current_binding = carbon_binding;
        ShadowPlan::Shadow { shadow, degraded }
    }

    /// Registrations match only when the callback id and Carbon string match.
    fn same_shadow(a: &ShortcutBinding, b: &ShortcutBinding) -> bool {
        a.id == b.id && a.current_binding == b.current_binding
    }

    /// Reconcile fallback registrations without replacing unchanged shadows.
    /// The operation mutex serializes reconciliations; fallback state is
    /// unlocked around plugin calls to avoid lock-order inversion.
    ///
    /// Carbon sends a release only to the registration that received the press.
    /// Replacing a held push-to-talk registration loses its release. See #1999.
    pub fn reconcile_fallback(app: &AppHandle) {
        let state = app.state::<SecureInputState>();
        let _operation = state.fallback_operation.lock().unwrap();

        let previous = {
            let mut fallback = state.fallback.lock().unwrap();
            std::mem::take(&mut *fallback)
        };

        let settings = settings::get_settings(app);
        let eligible = state.is_observed_enabled()
            && app
                .try_state::<crate::commands::ShortcutsInitialized>()
                .is_some()
            && settings.keyboard_implementation == KeyboardImplementation::HandyKeys;
        let ctx = FallbackContext {
            armed: eligible,
            suspended: state.shortcuts_suspended.load(Ordering::SeqCst),
            cancel_requested: state.cancel_requested.load(Ordering::SeqCst),
            post_process_enabled: settings.post_process_enabled,
        };

        let mut next = FallbackState::default();
        let mut immune = 0usize;
        let mut wanted: Vec<(String, ShortcutBinding, bool)> = Vec::new();
        for id in fallback_candidates(&settings.bindings, ctx) {
            let binding = &settings.bindings[&id];
            match plan_fallback_binding(&id, binding) {
                ShadowPlan::Immune => immune += 1,
                ShadowPlan::Uncovered => next.uncovered.push(id),
                ShadowPlan::Shadow { shadow, degraded } => wanted.push((id, shadow, degraded)),
            }
        }

        // Preserve unchanged registrations; unregister only stale shadows.
        let (kept, stale): (Vec<ShortcutBinding>, Vec<ShortcutBinding>) =
            previous.registered.into_iter().partition(|prev| {
                wanted
                    .iter()
                    .any(|(_, shadow, _)| same_shadow(shadow, prev))
            });

        if !stale.is_empty() {
            info!(
                "SecureInput fallback reconciling: removing {} Carbon shadow(s), keeping {}",
                stale.len(),
                kept.len()
            );
        }
        for binding in stale {
            if let Err(e) = crate::shortcut::tauri_impl::unregister_shortcut(app, binding.clone()) {
                warn!(
                    "SecureInput fallback: failed to unregister '{}': {}",
                    binding.current_binding, e
                );
            }
        }

        for (id, shadow, degraded) in wanted {
            if kept.iter().any(|k| same_shadow(k, &shadow)) {
                debug!(
                    "SecureInput fallback: '{}' still registered via Carbon as '{}', left untouched",
                    id, shadow.current_binding
                );
                next.registered.push(shadow);
                if degraded {
                    next.degraded.push(id);
                } else {
                    next.covered.push(id);
                }
                continue;
            }

            match crate::shortcut::tauri_impl::register_shortcut(app, shadow.clone()) {
                Ok(()) => {
                    info!(
                        "SecureInput fallback: '{}' registered via Carbon as '{}'{}",
                        id,
                        shadow.current_binding,
                        if degraded {
                            " (widened to either side)"
                        } else {
                            ""
                        }
                    );
                    next.registered.push(shadow);
                    if degraded {
                        next.degraded.push(id);
                    } else {
                        next.covered.push(id);
                    }
                }
                Err(e) => {
                    warn!(
                        "SecureInput fallback: could not cover '{}' ('{}'): {}",
                        id, shadow.current_binding, e
                    );
                    next.uncovered.push(id);
                }
            }
        }

        if eligible {
            info!(
                "SecureInput fallback active: {} covered, {} degraded, {} uncovered, {} immune{} (user impact: {})",
                next.covered.len(),
                next.degraded.len(),
                next.uncovered.len(),
                immune,
                if ctx.suspended {
                    ", suspended while recording a shortcut"
                } else {
                    ""
                },
                !next.degraded.is_empty() || !next.uncovered.is_empty()
            );
        } else if state.is_observed_enabled()
            && app
                .try_state::<crate::commands::ShortcutsInitialized>()
                .is_none()
        {
            debug!("SecureInput fallback deferred until shortcuts are initialized");
        }

        *state.fallback.lock().unwrap() = next;
        drop(_operation);

        // The tray sync diffs against what is displayed, so this is free when
        // the warning state did not change. Lock is released first: the sync
        // reads app state and must not nest under the operation mutex.
        refresh_tray(app);
        emit_status(app);
    }

    fn schedule_reconcile(app: &AppHandle) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            reconcile_fallback(&app);
        });
    }

    pub fn register_cancel_fallback(app: &AppHandle) {
        let state = app.state::<SecureInputState>();
        state.cancel_requested.store(true, Ordering::SeqCst);
        // Without observed Secure Input there are no Carbon shadows to
        // update. The monitor performs reconciliation when secure input is
        // observed or released, so spawning here would only race the normal
        // tray state transition for every recording start.
        if state.is_observed_enabled() {
            schedule_reconcile(app);
        }
    }

    pub fn unregister_cancel_fallback(app: &AppHandle) {
        let state = app.state::<SecureInputState>();
        state.cancel_requested.store(false, Ordering::SeqCst);
        if state.is_observed_enabled() {
            schedule_reconcile(app);
        }
    }

    /// Count-only capture test for the debug window. Opens a short-lived
    /// keyboard listener and tallies event kinds; key identity is never
    /// inspected beyond the mouse/keyboard distinction, and nothing about
    /// individual events is logged or returned.
    pub async fn run_diagnostic(duration_secs: u32) -> Result<KeyboardDiagnosticReport, String> {
        tauri::async_runtime::spawn_blocking(move || {
            let listener = handy_keys::KeyboardListener::new()
                .map_err(|e| format!("Failed to create keyboard listener: {e}"))?;

            let enabled_at_start = is_enabled();
            let start = Instant::now();
            let deadline = start + Duration::from_secs(duration_secs as u64);
            let (mut key_down, mut key_up, mut flags_changed, mut mouse) = (0u32, 0u32, 0u32, 0u32);

            while Instant::now() < deadline {
                match listener.try_recv() {
                    Some(event) => match &event.key {
                        Some(k) if is_mouse_key(k) => mouse += 1,
                        Some(_) if event.is_key_down => key_down += 1,
                        Some(_) => key_up += 1,
                        None => flags_changed += 1,
                    },
                    None => std::thread::sleep(Duration::from_millis(10)),
                }
            }

            let enabled = enabled_at_start || is_enabled();
            let culprit = if enabled { lookup_culprit() } else { None };
            info!(
                "keyboard diagnostic: secure_input={} key_down={} key_up={} flags_changed={} mouse={}",
                enabled, key_down, key_up, flags_changed, mouse
            );

            Ok(KeyboardDiagnosticReport {
                secure_input_enabled: enabled,
                culprit_pid: culprit.as_ref().map(|c| c.pid),
                culprit_name: culprit.map(|c| c.name),
                key_down,
                key_up,
                flags_changed,
                mouse,
                duration_ms: start.elapsed().as_millis() as u32,
            })
        })
        .await
        .map_err(|e| format!("Diagnostic task failed: {e}"))?
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub struct SecureInputState;

    impl SecureInputState {
        pub fn new() -> Self {
            Self
        }
        pub fn warning_active(&self) -> bool {
            false
        }
    }

    pub fn is_enabled() -> bool {
        false
    }

    pub fn start_monitor(_app: &AppHandle) {}

    pub fn status(_app: &AppHandle) -> SecureInputStatus {
        SecureInputStatus {
            enabled: false,
            sustained: false,
            culprit_pid: None,
            culprit_name: None,
            fallback_active: false,
            covered_bindings: Vec::new(),
            degraded_bindings: Vec::new(),
            uncovered_bindings: Vec::new(),
        }
    }

    pub fn set_shortcuts_suspended(_app: &AppHandle, _suspended: bool) {}

    pub fn register_cancel_fallback(_app: &AppHandle) {}

    pub fn unregister_cancel_fallback(_app: &AppHandle) {}

    pub fn reconcile_fallback(_app: &AppHandle) {}

    pub async fn run_diagnostic(_duration_secs: u32) -> Result<KeyboardDiagnosticReport, String> {
        Err("The keyboard diagnostic is only supported on macOS".to_string())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::imp::*;
    use crate::settings::ShortcutBinding;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    fn binding(id: &str, current: &str) -> ShortcutBinding {
        ShortcutBinding {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            default_binding: current.to_string(),
            current_binding: current.to_string(),
        }
    }

    /// The user's bindings from the reported log.
    fn user_bindings() -> HashMap<String, ShortcutBinding> {
        [
            binding("transcribe", "option+space"),
            binding("transcribe_with_post_process", "option+shift+space"),
            binding("cancel", "escape"),
        ]
        .into_iter()
        .map(|b| (b.id.clone(), b))
        .collect()
    }

    fn ctx() -> FallbackContext {
        FallbackContext {
            armed: true,
            suspended: false,
            cancel_requested: false,
            post_process_enabled: true,
        }
    }

    /// Chrome episode from the log: enabled, held, released, enabled again.
    #[test]
    fn fallback_arms_on_the_first_enabled_sample_and_warns_only_after_the_threshold() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut tracker = EpisodeTracker::default();

        assert_eq!(tracker.observe(false, at(0)), EpisodeStep::default());

        let first = tracker.observe(true, at(1));
        assert!(first.became_enabled, "fallback must arm without delay");
        assert!(!first.became_sustained);

        assert_eq!(tracker.observe(true, at(2)), EpisodeStep::default());
        assert_eq!(tracker.observe(true, at(3)), EpisodeStep::default());
        let sustained = tracker.observe(true, at(4));
        assert!(sustained.became_sustained, "warning gate opens at 3 s");
        assert!(!sustained.became_enabled);
        // Long holds (Chrome kept it for minutes) do not re-fire transitions.
        assert_eq!(tracker.observe(true, at(200)), EpisodeStep::default());

        let released = tracker.observe(false, at(201));
        assert!(released.became_disabled, "release must clear the warning");
        assert_eq!(tracker.observe(false, at(202)), EpisodeStep::default());

        // The next episode starts fresh: re-armed at once, re-warned only
        // after another full threshold.
        assert!(tracker.observe(true, at(203)).became_enabled);
        assert_eq!(tracker.observe(true, at(205)), EpisodeStep::default());
        assert!(tracker.observe(true, at(206)).became_sustained);
    }

    #[test]
    fn short_episode_released_before_the_threshold_never_warns() {
        let t0 = Instant::now();
        let mut tracker = EpisodeTracker::default();
        assert!(tracker.observe(true, t0).became_enabled);
        let step = tracker.observe(false, t0 + Duration::from_secs(2));
        assert!(step.became_disabled);
        assert!(!step.became_sustained);
    }

    #[test]
    fn both_transcribe_bindings_are_shadowed_while_armed() {
        let ids = fallback_candidates(&user_bindings(), ctx());
        assert_eq!(ids, vec!["transcribe", "transcribe_with_post_process"]);
        for id in &ids {
            match plan_fallback_binding(id, &user_bindings()[id]) {
                ShadowPlan::Shadow { shadow, degraded } => {
                    assert!(!degraded, "{id} keeps identical semantics");
                    assert_eq!(shadow.current_binding, user_bindings()[id].current_binding);
                }
                _ => panic!("{id} should be shadowed through Carbon"),
            }
        }
    }

    #[test]
    fn nothing_is_shadowed_before_secure_input_is_observed() {
        let ids = fallback_candidates(
            &user_bindings(),
            FallbackContext {
                armed: false,
                cancel_requested: true,
                ..ctx()
            },
        );
        assert!(ids.is_empty());
    }

    /// Recording a shortcut from the settings window while Chrome holds
    /// Secure Input: a live Carbon `option+space` would start dictation and
    /// swallow the very keys being recorded.
    #[test]
    fn recording_a_shortcut_drops_the_transcribe_shadows() {
        let ids = fallback_candidates(
            &user_bindings(),
            FallbackContext {
                suspended: true,
                ..ctx()
            },
        );
        assert!(ids.is_empty());

        // Cancel follows the audio-recording lifecycle, not the recorder.
        let ids = fallback_candidates(
            &user_bindings(),
            FallbackContext {
                suspended: true,
                cancel_requested: true,
                ..ctx()
            },
        );
        assert_eq!(ids, vec!["cancel"]);
    }

    #[test]
    fn post_process_shadow_follows_the_feature_toggle() {
        let ids = fallback_candidates(
            &user_bindings(),
            FallbackContext {
                post_process_enabled: false,
                ..ctx()
            },
        );
        assert_eq!(ids, vec!["transcribe"]);
    }
}
