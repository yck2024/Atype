use std::collections::{HashMap, HashSet, VecDeque};

use handy_keys::{
    BlockingHotkeys, Error, Hotkey, HotkeyState, KeyEvent, KeyboardListener, Modifiers, Result,
};

use super::{is_macos_f1, without_synthetic_fn};

pub type HotkeyId = u32;

pub struct HotkeyEvent {
    pub id: HotkeyId,
    pub state: HotkeyState,
}

#[derive(Default)]
struct Bindings {
    hotkeys: HashMap<HotkeyId, (Hotkey, bool)>,
    next_id: HotkeyId,
}

impl Bindings {
    fn register(&mut self, blocked: &mut HashSet<Hotkey>, hotkey: Hotkey) -> Result<HotkeyId> {
        let hotkey = without_synthetic_fn(hotkey);
        if !blocked.insert(hotkey) {
            return Err(Error::HotkeyAlreadyRegistered(hotkey.to_string()));
        }
        if is_macos_f1(hotkey) {
            blocked.insert(Hotkey {
                modifiers: hotkey.modifiers | Modifiers::FN,
                ..hotkey
            });
        }
        let id = self.next_id;
        self.next_id += 1;
        self.hotkeys.insert(id, (hotkey, false));
        Ok(id)
    }

    fn unregister(&mut self, blocked: &mut HashSet<Hotkey>, id: HotkeyId) {
        if let Some((hotkey, _)) = self.hotkeys.remove(&id) {
            blocked.remove(&hotkey);
            if is_macos_f1(hotkey) {
                blocked.remove(&Hotkey {
                    modifiers: hotkey.modifiers | Modifiers::FN,
                    ..hotkey
                });
            }
        }
    }

    fn process(&mut self, event: KeyEvent) -> impl Iterator<Item = HotkeyEvent> + '_ {
        self.hotkeys
            .iter_mut()
            .filter_map(move |(&id, (hotkey, pressed))| {
                let modifiers = if is_macos_f1(*hotkey) {
                    event.modifiers - Modifiers::FN
                } else {
                    event.modifiers
                };
                let key_matches = event.key == hotkey.key;
                let state = if event.is_key_down {
                    if *pressed || !key_matches || !hotkey.modifiers.matches(modifiers) {
                        return None;
                    }
                    HotkeyState::Pressed
                } else {
                    let held_modifiers = if is_macos_f1(*hotkey) {
                        modifiers & hotkey.modifiers
                    } else {
                        modifiers
                    };
                    if !*pressed
                        || !(if event.key.is_some() {
                            key_matches
                        } else {
                            !hotkey.modifiers.matches(held_modifiers)
                        })
                    {
                        return None;
                    }
                    HotkeyState::Released
                };
                *pressed = state == HotkeyState::Pressed;
                Some(HotkeyEvent { id, state })
            })
    }
}

pub struct HotkeyManager {
    listener: KeyboardListener,
    blocked: BlockingHotkeys,
    bindings: Bindings,
    events: VecDeque<HotkeyEvent>,
}

impl HotkeyManager {
    pub fn new_with_blocking() -> Result<Self> {
        let blocked = BlockingHotkeys::default();
        Ok(Self {
            listener: KeyboardListener::new_with_blocking(blocked.clone())?,
            blocked,
            bindings: Bindings::default(),
            events: VecDeque::new(),
        })
    }

    pub fn register(&mut self, hotkey: Hotkey) -> Result<HotkeyId> {
        let mut blocked = self.blocked.lock().map_err(|_| Error::MutexPoisoned)?;
        self.bindings.register(&mut blocked, hotkey)
    }

    pub fn unregister(&mut self, id: HotkeyId) -> Result<()> {
        let mut blocked = self.blocked.lock().map_err(|_| Error::MutexPoisoned)?;
        self.bindings.unregister(&mut blocked, id);
        Ok(())
    }

    pub fn try_recv(&mut self) -> Option<HotkeyEvent> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Some(event);
            }
            self.events
                .extend(self.bindings.process(self.listener.try_recv()?));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use handy_keys::Key;

    fn key(modifiers: Modifiers, key: Key, is_key_down: bool) -> KeyEvent {
        KeyEvent {
            modifiers,
            key: Some(key),
            is_key_down,
            changed_modifier: None,
        }
    }

    fn release_modifier(modifiers: Modifiers, changed_modifier: Modifiers) -> KeyEvent {
        KeyEvent {
            modifiers,
            key: None,
            is_key_down: false,
            changed_modifier: Some(changed_modifier),
        }
    }

    #[test]
    fn f1_stays_pressed_until_key_up_with_or_without_fn() {
        use Modifiers as M;
        for binding in ["f1", "fn+f1"] {
            for flags in [M::empty(), M::FN] {
                let mut bindings = Bindings::default();
                let mut blocked = HashSet::new();
                let id = bindings
                    .register(&mut blocked, binding.parse().unwrap())
                    .unwrap();
                for event in [
                    key(flags, Key::F1, true),
                    key(flags ^ M::FN, Key::F1, false),
                ] {
                    assert!(blocked.iter().any(|hotkey| hotkey.key == event.key
                        && hotkey.modifiers.matches(event.modifiers)));
                }
                assert!(bindings
                    .register(&mut blocked, "f1".parse().unwrap())
                    .is_err());
                let press = bindings.process(key(flags, Key::F1, true)).next().unwrap();
                assert_eq!((press.id, press.state), (id, HotkeyState::Pressed));
                assert!(bindings.process(key(flags, Key::F1, true)).next().is_none());
                assert!(bindings
                    .process(release_modifier(M::empty(), M::SHIFT_LEFT))
                    .next()
                    .is_none());
                assert!(bindings
                    .process(release_modifier(M::CTRL_LEFT, M::SHIFT_LEFT))
                    .next()
                    .is_none());
                let release = bindings
                    .process(key(flags ^ M::FN, Key::F1, false))
                    .next()
                    .unwrap();
                assert_eq!((release.id, release.state), (id, HotkeyState::Released));
                bindings.unregister(&mut blocked, id);
                assert!(blocked.is_empty());
            }
        }
    }

    #[test]
    fn f1_still_releases_when_a_required_modifier_is_released() {
        use Modifiers as M;
        let mut bindings = Bindings::default();
        bindings
            .register(&mut HashSet::new(), "shift+f1".parse().unwrap())
            .unwrap();
        assert!(bindings.process(key(M::FN, Key::F1, true)).next().is_none());
        assert_eq!(
            bindings
                .process(key(M::SHIFT_LEFT | M::FN, Key::F1, true))
                .next()
                .unwrap()
                .state,
            HotkeyState::Pressed
        );
        assert_eq!(
            bindings
                .process(release_modifier(M::FN, M::SHIFT_LEFT))
                .next()
                .unwrap()
                .state,
            HotkeyState::Released
        );
        assert!(bindings
            .process(key(M::empty(), Key::F1, false))
            .next()
            .is_none());
    }

    #[test]
    fn other_keys_still_match_fn_exactly() {
        use Modifiers as M;
        let mut bindings = Bindings::default();
        let mut blocked = HashSet::new();
        bindings
            .register(&mut blocked, "fn+f2".parse().unwrap())
            .unwrap();
        assert_eq!(blocked, HashSet::from(["fn+f2".parse().unwrap()]));
        assert!(bindings
            .process(key(M::empty(), Key::F2, true))
            .next()
            .is_none());
        assert_eq!(
            bindings
                .process(key(M::FN, Key::F2, true))
                .next()
                .unwrap()
                .state,
            HotkeyState::Pressed
        );
        assert_eq!(
            bindings
                .process(release_modifier(M::empty(), M::FN))
                .next()
                .unwrap()
                .state,
            HotkeyState::Released
        );
    }
}
