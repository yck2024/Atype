/**
 * Shortcut capture from the settings window's own key events.
 *
 * The handy-keys recorder listens through a global event tap. While another
 * app holds macOS Secure Input (for example Chrome with a password field
 * focused), that tap receives no KeyDown/KeyUp, so the recorder cannot see
 * keyed shortcuts. Secure Input does not stop key events from reaching the
 * focused window, so the settings window can still record them itself.
 *
 * Output strings use handy-keys names (see `Key::from_str` in the handy-keys
 * crate and the `window_capture_strings_parse_as_keyed_hotkeys` Rust test).
 * Modifiers are recorded without a side (`option`, not `option_left`) because
 * DOM modifier flags do not say which side is held.
 *
 * Not available in this mode: modifier-only shortcuts and fn combinations.
 */

export interface WindowKeyEvent {
  code: string;
  repeat?: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  getModifierState?: (key: string) => boolean;
}

export type WindowCaptureAction =
  /** Nothing to do. */
  | { type: "ignore" }
  /** Show this in-progress combination (may be modifiers only). */
  | { type: "preview"; hotkey: string }
  /** Save this keyed combination and stop recording. */
  | { type: "commit"; hotkey: string }
  /** The key cannot be recorded in this mode (fn, unknown key). */
  | { type: "unsupported" };

const MODIFIER_CODES = new Set([
  "ShiftLeft",
  "ShiftRight",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "MetaLeft",
  "MetaRight",
  "OSLeft",
  "OSRight",
  "Fn",
  "FnLock",
  "CapsLock",
]);

const CODE_TO_KEY: Record<string, string> = {
  Space: "space",
  Enter: "return",
  Tab: "tab",
  Escape: "escape",
  // macOS: Backspace is "Delete", Delete is "Forward Delete"
  Backspace: "delete",
  Delete: "forwarddelete",
  Insert: "insert",
  Home: "home",
  End: "end",
  PageUp: "pageup",
  PageDown: "pagedown",
  ArrowLeft: "left",
  ArrowRight: "right",
  ArrowUp: "up",
  ArrowDown: "down",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backquote: "`",
  IntlBackslash: "§",
  NumpadDecimal: "keypaddecimal",
  NumpadMultiply: "keypadmultiply",
  NumpadAdd: "keypadplus",
  NumpadSubtract: "keypadminus",
  NumpadDivide: "keypaddivide",
  NumpadEnter: "keypadenter",
  NumpadEqual: "keypadequals",
  NumpadComma: "keypadcomma",
  // The Mac keypad's Clear key reports the NumLock code
  NumLock: "keypadclear",
  Lang1: "kana",
  Lang2: "eisu",
};

/** handy-keys name for a DOM `KeyboardEvent.code`, or null if unsupported. */
export const windowKeyName = (code: string): string | null => {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase();
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code.toLowerCase();
  if (/^Numpad[0-9]$/.test(code)) return `keypad${code.slice(6)}`;
  return CODE_TO_KEY[code] ?? null;
};

/** Held modifiers in handy-keys order (ctrl, option, shift, command). */
export const windowModifiers = (e: WindowKeyEvent): string[] => {
  const modifiers: string[] = [];
  if (e.ctrlKey) modifiers.push("ctrl");
  if (e.altKey) modifiers.push("option");
  if (e.shiftKey) modifiers.push("shift");
  if (e.metaKey) modifiers.push("command");
  return modifiers;
};

const fnHeld = (e: WindowKeyEvent): boolean =>
  e.code === "Fn" || e.getModifierState?.("Fn") === true;

/**
 * Turns one recording session's window key events into preview/commit
 * actions. A keyed combination commits when its main key is released, or
 * when the last modifier is released first (macOS does not deliver the main
 * key's keyup while Command is held).
 */
export class WindowShortcutCapture {
  private keyed = "";
  private mainCode = "";

  keyDown(e: WindowKeyEvent): WindowCaptureAction {
    if (e.repeat) return { type: "ignore" };
    if (fnHeld(e)) return { type: "unsupported" };

    if (MODIFIER_CODES.has(e.code)) {
      if (this.keyed) return { type: "ignore" };
      return { type: "preview", hotkey: windowModifiers(e).join("+") };
    }

    const key = windowKeyName(e.code);
    if (!key) return { type: "unsupported" };

    this.keyed = [...windowModifiers(e), key].join("+");
    this.mainCode = e.code;
    return { type: "preview", hotkey: this.keyed };
  }

  keyUp(e: WindowKeyEvent): WindowCaptureAction {
    const modifiersLeft = windowModifiers(e).length;

    if (!this.keyed) {
      // Modifier-only combinations cannot be recorded in this mode; just
      // keep the preview in sync with what is still held.
      return { type: "preview", hotkey: windowModifiers(e).join("+") };
    }

    if (
      e.code === this.mainCode ||
      (MODIFIER_CODES.has(e.code) && modifiersLeft === 0)
    ) {
      const hotkey = this.keyed;
      this.keyed = "";
      this.mainCode = "";
      return { type: "commit", hotkey };
    }

    return { type: "ignore" };
  }
}

/** The backend refused the event-tap recorder because of Secure Input. */
export const isSecureInputRefusal = (error: unknown): boolean =>
  String(error).includes("secure-input-active");
