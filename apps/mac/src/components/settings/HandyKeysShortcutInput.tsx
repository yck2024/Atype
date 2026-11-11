import React, { useEffect, useState, useRef, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { formatKeyCombination } from "../../lib/utils/keyboard";
import {
  isSecureInputRefusal,
  WindowShortcutCapture,
  type WindowCaptureAction,
} from "../../lib/utils/windowShortcutCapture";
import { ResetButton } from "../ui/ResetButton";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import { useOsType } from "../../hooks/useOsType";
import { commands } from "@/bindings";
import { toast } from "sonner";

interface HandyKeysShortcutInputProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  shortcutId: string;
  disabled?: boolean;
}

interface HandyKeysEvent {
  modifiers: string[];
  key: string | null;
  is_key_down: boolean;
  hotkey_string: string;
}

/**
 * "tap": the backend's global event tap (sees modifier-only and fn combos).
 * "window": this window's own key events, used while another app holds
 * macOS Secure Input and the tap is blind to key presses.
 */
type CaptureMode = "tap" | "window";

let activeCaptureOwner: object | null = null;

export const HandyKeysShortcutInput: React.FC<HandyKeysShortcutInputProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
  shortcutId,
  disabled = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateBinding, resetBinding, isUpdating, isLoading } =
    useSettings();
  const [isRecording, setIsRecording] = useState(false);
  const [captureMode, setCaptureMode] = useState<CaptureMode>("tap");
  // App holding Secure Input, shown in the window-capture hint
  const [secureInputHolder, setSecureInputHolder] = useState<string | null>(
    null,
  );
  const [currentKeys, setCurrentKeys] = useState<string>("");
  const [originalBinding, setOriginalBinding] = useState<string>("");
  const shortcutRef = useRef<HTMLDivElement | null>(null);
  const unlistenRef = useRef<(() => void) | null>(null);
  // Mirrors isRecording for unmount cleanup
  const recordingRef = useRef(false);
  // One start attempt at a time: a double click used to fire two refused
  // attempts and stack two identical toasts.
  const startingRef = useRef(false);
  const captureOwnerRef = useRef<object>({});
  const mountedRef = useRef(true);
  // Use a ref to track currentKeys for the event handler (avoids stale closure)
  const currentKeysRef = useRef<string>("");
  // Track keyed vs modifier-only captures separately so a combo commits only
  // on its key's release and a modifier-only shortcut only once every
  // modifier is released. Committing on the *first* release (the old
  // behavior) silently saved just the modifier whenever the key event never
  // arrived — e.g. while macOS Secure Input is active (issue #1578).
  const keyedShortcutRef = useRef<string>("");
  const modifierOnlyShortcutRef = useRef<string>("");
  const osType = useOsType();

  const bindings = getSetting("bindings") || {};

  const releaseCaptureOwner = useCallback(() => {
    if (activeCaptureOwner === captureOwnerRef.current) {
      activeCaptureOwner = null;
    }
  }, []);

  const resetCaptureRefs = useCallback(() => {
    currentKeysRef.current = "";
    keyedShortcutRef.current = "";
    modifierOnlyShortcutRef.current = "";
  }, []);

  const stopListening = useCallback(() => {
    if (unlistenRef.current) {
      unlistenRef.current();
      unlistenRef.current = null;
    }
  }, []);

  // Stop the backend recorder (if any) and re-register every shortcut,
  // including Secure Input's Carbon shadows, then leave recording mode.
  const finishRecording = useCallback(async () => {
    stopListening();
    await commands.stopHandyKeysRecording().catch(console.error);
    recordingRef.current = false;
    setIsRecording(false);
    setCaptureMode("tap");
    setSecureInputHolder(null);
    setCurrentKeys("");
    resetCaptureRefs();
    setOriginalBinding("");
    releaseCaptureOwner();
  }, [stopListening, resetCaptureRefs, releaseCaptureOwner]);

  // Handle cancellation
  const cancelRecording = useCallback(async () => {
    if (!isRecording) return;

    // Stop listening for key events; finishRecording stops the backend
    stopListening();

    // Restore original binding
    if (originalBinding) {
      try {
        await updateBinding(shortcutId, originalBinding);
      } catch (error) {
        console.error("Failed to restore original binding:", error);
        toast.error(t("settings.general.shortcut.errors.restore"));
      }
    }

    await finishRecording();
  }, [
    isRecording,
    originalBinding,
    shortcutId,
    updateBinding,
    t,
    stopListening,
    finishRecording,
  ]);

  const commitAndStop = useCallback(
    async (keysToCommit: string) => {
      try {
        await updateBinding(shortcutId, keysToCommit);
      } catch (error) {
        console.error("Failed to change binding:", error);
        toast.error(
          t("settings.general.shortcut.errors.set", {
            error: String(error),
          }),
        );

        // Reset to original binding on error
        if (originalBinding) {
          try {
            await updateBinding(shortcutId, originalBinding);
          } catch (resetError) {
            console.error("Failed to reset binding:", resetError);
            toast.error(t("settings.general.shortcut.errors.reset"));
          }
        }
      }

      await finishRecording();
    },
    [shortcutId, originalBinding, updateBinding, t, finishRecording],
  );

  // Event handlers read the latest callback through a ref, so a re-render
  // never tears down a capture that is half-way through a combination.
  const commitRef = useRef(commitAndStop);
  commitRef.current = commitAndStop;

  // Record from this window's own key events (Secure Input fallback).
  const enterWindowCapture = useCallback(async () => {
    stopListening();
    resetCaptureRefs();
    setCurrentKeys("");
    setCaptureMode("window");
    recordingRef.current = true;
    setIsRecording(true);
    try {
      const status = await commands.getSecureInputStatus();
      setSecureInputHolder(status.culprit_name);
    } catch (e) {
      console.warn("Failed to fetch secure input status:", e);
    }
  }, [stopListening, resetCaptureRefs]);

  // Event-tap capture: key events come from the backend listener
  useEffect(() => {
    if (!isRecording || captureMode !== "tap") return;

    let cleanup = false;

    const setupListener = async () => {
      const unlisten = await listen<HandyKeysEvent>(
        "handy-keys-event",
        async (event) => {
          if (cleanup) return;

          const { hotkey_string, is_key_down, key, modifiers } = event.payload;

          if (is_key_down && hotkey_string) {
            // Update both state (for display) and refs (for release handler)
            if (key) {
              keyedShortcutRef.current = hotkey_string;
            } else {
              modifierOnlyShortcutRef.current = hotkey_string;
            }
            currentKeysRef.current = hotkey_string;
            setCurrentKeys(hotkey_string);
          } else if (!is_key_down && key) {
            // The main key was released — commit the keyed combo. The release
            // event's hotkey_string still contains the key, so it works even
            // if the key-down was somehow missed. Never fall back to a
            // modifier-only capture here: that's how bindings used to get
            // silently overwritten with just the modifier (issue #1578).
            const keysToCommit = keyedShortcutRef.current || hotkey_string;
            if (keysToCommit) {
              cleanup = true;
              await commitRef.current(keysToCommit);
            }
          } else if (
            !is_key_down &&
            !key &&
            modifiers.length === 0 &&
            !keyedShortcutRef.current &&
            modifierOnlyShortcutRef.current
          ) {
            const modifierOnly = modifierOnlyShortcutRef.current;
            cleanup = true;
            // If Secure Input switched on mid-capture, the tap only saw the
            // modifier of a keyed combo. Keep recording from the window
            // instead of saving just the modifier.
            const status = await commands
              .getSecureInputStatus()
              .catch(() => null);
            if (status?.enabled) {
              await enterWindowCapture();
              return;
            }
            // Every modifier released without a main key ever going down —
            // commit as a modifier-only shortcut
            await commitRef.current(modifierOnly);
          }
        },
      );

      if (cleanup) {
        unlisten();
      } else {
        unlistenRef.current = unlisten;
      }
    };

    setupListener();

    return () => {
      cleanup = true;
      stopListening();
    };
  }, [isRecording, captureMode, enterWindowCapture, stopListening]);

  // Window capture: key events come from this window while the backend's
  // tap is blind (another app holds macOS Secure Input).
  useEffect(() => {
    if (!isRecording || captureMode !== "window") return;

    const capture = new WindowShortcutCapture();
    let committed = false;

    const apply = (action: WindowCaptureAction) => {
      if (committed) return;
      if (action.type === "preview") {
        setCurrentKeys(action.hotkey);
      } else if (action.type === "commit") {
        committed = true;
        void commitRef.current(action.hotkey);
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      apply(capture.keyDown(e));
    };
    const onKeyUp = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      apply(capture.keyUp(e));
    };

    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
    };
  }, [isRecording, captureMode]);

  // Stop backend recording on unmount to prevent orphaned recording loops
  // and leave every shortcut registered.
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      stopListening();
      if (recordingRef.current) {
        commands.stopHandyKeysRecording().catch(console.error);
        releaseCaptureOwner();
      } else if (!startingRef.current) {
        releaseCaptureOwner();
      }
    };
  }, [stopListening, releaseCaptureOwner]);

  // Handle click outside
  useEffect(() => {
    if (!isRecording) return;

    const handleClickOutside = (e: MouseEvent) => {
      if (
        shortcutRef.current &&
        !shortcutRef.current.contains(e.target as Node)
      ) {
        cancelRecording();
      }
    };

    window.addEventListener("click", handleClickOutside);
    return () => window.removeEventListener("click", handleClickOutside);
  }, [isRecording, cancelRecording]);

  // Start recording a new shortcut
  const startRecording = async () => {
    if (
      isRecording ||
      startingRef.current ||
      (activeCaptureOwner !== null &&
        activeCaptureOwner !== captureOwnerRef.current)
    ) {
      return;
    }
    activeCaptureOwner = captureOwnerRef.current;
    startingRef.current = true;

    // Store the original binding to restore if canceled
    setOriginalBinding(bindings[shortcutId]?.current_binding || "");

    try {
      const result = await commands.startHandyKeysRecording(shortcutId);
      if (
        !mountedRef.current ||
        activeCaptureOwner !== captureOwnerRef.current
      ) {
        if (result.status === "ok") {
          await commands.stopHandyKeysRecording().catch(console.error);
        }
        releaseCaptureOwner();
        return;
      }
      if (result.status === "error") {
        if (isSecureInputRefusal(result.error)) {
          // Another app holds macOS Secure Input, so the backend's event
          // tap cannot see key presses. This window still receives its own
          // key events: suspend every shortcut (Carbon shadows included, so
          // none fires or swallows the keys) and record from here.
          await commands.suspendAllBindings().catch(console.error);
          await enterWindowCapture();
        } else {
          toast.error(
            t("settings.general.shortcut.errors.set", {
              error: String(result.error),
            }),
          );
        }
        releaseCaptureOwner();
        return;
      }
      resetCaptureRefs();
      setCurrentKeys("");
      setCaptureMode("tap");
      recordingRef.current = true;
      setIsRecording(true);
    } catch (error) {
      console.error("Failed to start recording:", error);
      toast.error(
        t("settings.general.shortcut.errors.set", { error: String(error) }),
      );
      releaseCaptureOwner();
    } finally {
      startingRef.current = false;
    }
  };

  // Format the current shortcut keys being recorded
  const formatCurrentKeys = (): string => {
    if (!currentKeys) return t("settings.general.shortcut.pressKeys");
    return formatKeyCombination(currentKeys, osType);
  };

  // If still loading, show loading state
  if (isLoading) {
    return (
      <SettingContainer
        title={t("settings.general.shortcut.title")}
        description={t("settings.general.shortcut.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <div className="text-sm text-mid-gray">
          {t("settings.general.shortcut.loading")}
        </div>
      </SettingContainer>
    );
  }

  // If no bindings are loaded, show empty state
  if (Object.keys(bindings).length === 0) {
    return (
      <SettingContainer
        title={t("settings.general.shortcut.title")}
        description={t("settings.general.shortcut.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <div className="text-sm text-mid-gray">
          {t("settings.general.shortcut.none")}
        </div>
      </SettingContainer>
    );
  }

  const binding = bindings[shortcutId];
  if (!binding) {
    return (
      <SettingContainer
        title={t("settings.general.shortcut.title")}
        description={t("settings.general.shortcut.notFound")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <div className="text-sm text-mid-gray">
          {t("settings.general.shortcut.none")}
        </div>
      </SettingContainer>
    );
  }

  // Get translated name and description for the binding
  const translatedName = t(
    `settings.general.shortcut.bindings.${shortcutId}.name`,
    binding.name,
  );
  const translatedDescription = t(
    `settings.general.shortcut.bindings.${shortcutId}.description`,
    binding.description,
  );

  return (
    <SettingContainer
      title={translatedName}
      description={translatedDescription}
      descriptionMode={descriptionMode}
      grouped={grouped}
      disabled={disabled}
      layout="horizontal"
    >
      <div className="flex items-center space-x-1">
        {isRecording ? (
          <div ref={shortcutRef} className="flex flex-col items-end gap-1">
            <div className="px-2 py-1 text-sm font-semibold border border-logo-primary bg-logo-primary/30 rounded-md">
              {formatCurrentKeys()}
            </div>
            {captureMode === "window" && (
              <p
                role="status"
                className="max-w-64 text-right text-xs leading-4 text-mid-gray"
              >
                {secureInputHolder
                  ? t(
                      "settings.general.shortcut.secureInputCaptureWithCulprit",
                      {
                        name: secureInputHolder,
                      },
                    )
                  : t("settings.general.shortcut.secureInputCapture")}
              </p>
            )}
          </div>
        ) : (
          <div
            className="px-2 py-1 text-sm font-semibold bg-mid-gray/10 border border-mid-gray/80 hover:bg-logo-primary/10 rounded-md cursor-pointer hover:border-logo-primary"
            onClick={startRecording}
          >
            {formatKeyCombination(binding.current_binding, osType)}
          </div>
        )}
        <ResetButton
          onClick={() => resetBinding(shortcutId)}
          disabled={isUpdating(`binding_${shortcutId}`)}
        />
      </div>
    </SettingContainer>
  );
};
