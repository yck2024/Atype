import assert from "node:assert/strict";
import {
  isSecureInputRefusal,
  windowKeyName,
  WindowShortcutCapture,
  type WindowKeyEvent,
} from "./windowShortcutCapture";

const ev = (
  code: string,
  mods: Partial<
    Pick<WindowKeyEvent, "ctrlKey" | "altKey" | "shiftKey" | "metaKey">
  > = {},
  extra: Partial<WindowKeyEvent> = {},
): WindowKeyEvent => ({
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
  ...extra,
});

// The backend's refusal marker switches the recorder to window capture
// instead of showing the old "Can't record shortcuts right now" toast.
assert.equal(isSecureInputRefusal("secure-input-active"), true);
assert.equal(isSecureInputRefusal("Already recording"), false);

// Option + Space, released in the natural order (the user's binding).
{
  const capture = new WindowShortcutCapture();
  assert.deepEqual(capture.keyDown(ev("AltLeft", { altKey: true })), {
    type: "preview",
    hotkey: "option",
  });
  assert.deepEqual(capture.keyDown(ev("Space", { altKey: true })), {
    type: "preview",
    hotkey: "option+space",
  });
  assert.deepEqual(capture.keyUp(ev("Space", { altKey: true })), {
    type: "commit",
    hotkey: "option+space",
  });
  // A later modifier release does not commit twice.
  assert.equal(capture.keyUp(ev("AltLeft")).type, "preview");
}

// Modifier released before the main key still commits the keyed combo.
{
  const capture = new WindowShortcutCapture();
  capture.keyDown(ev("AltRight", { altKey: true }));
  capture.keyDown(ev("Space", { altKey: true }));
  assert.deepEqual(capture.keyUp(ev("AltRight")), {
    type: "commit",
    hotkey: "option+space",
  });
}

// Command + key: macOS swallows the main key's keyup while Command is held,
// so the commit comes from releasing Command.
{
  const capture = new WindowShortcutCapture();
  capture.keyDown(ev("MetaLeft", { metaKey: true }));
  capture.keyDown(ev("ShiftLeft", { metaKey: true, shiftKey: true }));
  assert.deepEqual(
    capture.keyDown(ev("KeyK", { metaKey: true, shiftKey: true })),
    { type: "preview", hotkey: "shift+command+k" },
  );
  assert.equal(
    capture.keyUp(ev("ShiftLeft", { metaKey: true })).type,
    "ignore",
  );
  assert.deepEqual(capture.keyUp(ev("MetaLeft")), {
    type: "commit",
    hotkey: "shift+command+k",
  });
}

// Modifier order matches handy-keys' to_handy_string.
{
  const capture = new WindowShortcutCapture();
  assert.deepEqual(
    capture.keyDown(
      ev("Digit1", {
        metaKey: true,
        shiftKey: true,
        altKey: true,
        ctrlKey: true,
      }),
    ),
    { type: "preview", hotkey: "ctrl+option+shift+command+1" },
  );
}

// Modifier-only is not recordable here: releasing every modifier never
// commits (the old event-tap bug saved just the modifier).
{
  const capture = new WindowShortcutCapture();
  capture.keyDown(ev("AltLeft", { altKey: true }));
  assert.deepEqual(capture.keyUp(ev("AltLeft")), {
    type: "preview",
    hotkey: "",
  });
}

// fn combinations and unknown keys are refused rather than saved wrongly.
{
  const capture = new WindowShortcutCapture();
  assert.equal(
    capture.keyDown(ev("F5", {}, { getModifierState: (k) => k === "Fn" })).type,
    "unsupported",
  );
  assert.equal(capture.keyDown(ev("Fn")).type, "unsupported");
  assert.equal(capture.keyDown(ev("AudioVolumeUp")).type, "unsupported");
  assert.equal(capture.keyUp(ev("AudioVolumeUp")).type, "preview");
}

// Auto-repeat does not reset or re-commit.
{
  const capture = new WindowShortcutCapture();
  capture.keyDown(ev("KeyJ", { ctrlKey: true }));
  assert.equal(
    capture.keyDown(ev("KeyJ", { ctrlKey: true }, { repeat: true })).type,
    "ignore",
  );
  assert.deepEqual(capture.keyUp(ev("KeyJ", { ctrlKey: true })), {
    type: "commit",
    hotkey: "ctrl+j",
  });
}

// Every mapped name is one the Rust test
// `window_capture_strings_parse_as_keyed_hotkeys` proves handy-keys parses.
const expectedNames: Array<[string, string]> = [
  ["KeyA", "a"],
  ["Digit0", "0"],
  ["F1", "f1"],
  ["F24", "f24"],
  ["Numpad7", "keypad7"],
  ["Space", "space"],
  ["Enter", "return"],
  ["Escape", "escape"],
  ["Backspace", "delete"],
  ["Delete", "forwarddelete"],
  ["ArrowLeft", "left"],
  ["PageUp", "pageup"],
  ["Backslash", "\\"],
  ["Backquote", "`"],
  ["IntlBackslash", "§"],
  ["NumpadEnter", "keypadenter"],
  ["NumLock", "keypadclear"],
  ["Lang1", "kana"],
  ["Lang2", "eisu"],
];
for (const [code, name] of expectedNames) {
  assert.equal(windowKeyName(code), name, code);
}
assert.equal(windowKeyName("F25"), null);

console.log("windowShortcutCapture: all assertions passed");
