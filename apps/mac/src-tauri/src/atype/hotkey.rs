//! Function keys (F1–F20) as shortcuts on macOS.
//!
//! macOS sets the fn flag on every function-key event whether or not fn is
//! held, and handy-keys matches fn exactly. A stored `f1` would therefore
//! never fire, and the recorder would save `fn+f1`, which reads as "hold fn"
//! (on a Mac with standard function keys, fn+F1 is brightness instead).
//!
//! Atype stores function keys without fn ([`canonical`], used by the
//! recorder) and adds it back only when registering with the event tap
//! ([`for_tap`]). The Secure Input fallback reads the stored form, so `f1`
//! also gets a Carbon shadow, which `fn+f1` could not.
//!
//! Hooks into Handy code (each a one-line call):
//! - `shortcut::handy_keys` registration → [`for_tap`]
//! - `shortcut::handy_keys` recorder → [`canonical`]

use handy_keys::{Hotkey, Key, Modifiers};

fn is_function_key(key: &Key) -> bool {
    let name = key.to_string();
    name.len() > 1 && name.starts_with('F') && name[1..].parse::<u8>().is_ok()
}

/// Rewrite a function-key binding with or without fn; anything else
/// (including unparseable strings) is returned unchanged.
fn with_fn(binding: &str, fn_held: bool) -> String {
    let Ok(hotkey) = binding.parse::<Hotkey>() else {
        return binding.to_string();
    };
    if !hotkey.key.as_ref().is_some_and(is_function_key) {
        return binding.to_string();
    }
    let modifiers = if fn_held {
        hotkey.modifiers | Modifiers::FN
    } else {
        hotkey.modifiers - Modifiers::FN
    };
    if modifiers == hotkey.modifiers {
        return binding.to_string();
    }
    Hotkey::new(modifiers, hotkey.key)
        .map(|h| h.to_handy_string())
        .unwrap_or_else(|_| binding.to_string())
}

/// The form to store and show: `fn+f1` → `f1` on macOS.
pub fn canonical(binding: &str) -> String {
    if cfg!(target_os = "macos") {
        with_fn(binding, false)
    } else {
        binding.to_string()
    }
}

/// The form the macOS event tap actually sees: `f1` → `fn+f1`.
pub fn for_tap(binding: &str) -> String {
    if cfg!(target_os = "macos") {
        with_fn(binding, true)
    } else {
        binding.to_string()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn same(a: &str, b: &str) -> bool {
        a.parse::<Hotkey>().unwrap() == b.parse::<Hotkey>().unwrap()
    }

    #[test]
    fn function_keys_drop_fn_when_stored() {
        assert!(same(&canonical("fn+f1"), "f1"));
        assert!(same(&canonical("fn+shift+f12"), "shift+f12"));
        assert!(same(&canonical("fn+f20"), "f20"));
        assert_eq!(canonical("f1"), "f1");
    }

    #[test]
    fn function_keys_gain_fn_for_the_tap() {
        assert!(same(&for_tap("f1"), "fn+f1"));
        assert!(same(&for_tap("option+f5"), "fn+option+f5"));
        assert_eq!(for_tap("fn+f1"), "fn+f1");
    }

    #[test]
    fn other_bindings_are_untouched() {
        for b in [
            "option+space",
            "fn",
            "fn+space",
            "option_right+command_right",
            "escape",
            "f",
            "not a hotkey",
        ] {
            assert_eq!(canonical(b), b);
            assert_eq!(for_tap(b), b);
        }
    }
}
