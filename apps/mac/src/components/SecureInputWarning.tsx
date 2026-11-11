import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { ChevronDown, TriangleAlert, X } from "lucide-react";
import { commands, type SecureInputStatus } from "@/bindings";

/**
 * Compact warning banner shown while macOS Secure Input is stuck on.
 *
 * Secure Input (password fields, Terminal's "Secure Keyboard Entry", a stuck
 * loginwindow) blocks key events from reaching Atype's keyboard listener.
 * The backend moves keyed shortcuts to a Carbon fallback as soon as it sees
 * Secure Input, so this only appears when a shortcut is still affected
 * (side-specific modifiers widened, or fn/unregistrable combos dead) and
 * Secure Input has been held long enough (`sustained`) to not be a momentary
 * password-field focus. It clears as soon as Secure Input is released.
 */
const SecureInputWarning: React.FC = () => {
  const { t } = useTranslation();
  const [status, setStatus] = useState<SecureInputStatus | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [showFix, setShowFix] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setStatus(await commands.getSecureInputStatus());
    } catch (e) {
      console.warn("Failed to fetch secure input status:", e);
    }
  }, []);

  useEffect(() => {
    refresh();
    const unlisten = listen<SecureInputStatus>(
      "secure-input-changed",
      (event) => setStatus(event.payload),
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refresh]);

  // Only warn when the user is actually impacted: a binding is degraded
  // (side-specific matching widened) or dead (e.g. fn+key). When the
  // fallback covers everything transparently stay silent; the backend
  // still logs.
  const impacted =
    status !== null &&
    status.sustained &&
    (status.degraded_bindings.length > 0 ||
      status.uncovered_bindings.length > 0);

  // A dismissal lasts for the current episode only: once the condition
  // clears, the next occurrence warns again. The tray badge is the
  // persistent indicator and is not dismissible.
  useEffect(() => {
    if (!impacted) {
      setDismissed(false);
      setShowFix(false);
    }
  }, [impacted]);

  if (!impacted || dismissed) {
    return null;
  }

  const name = status.culprit_name;
  const affectedCount = new Set([
    ...status.uncovered_bindings,
    ...status.degraded_bindings,
  ]).size;
  const countSuffix = affectedCount === 1 ? "one" : "other";
  const message =
    name !== null
      ? t(`secureInput.blockedWithCulprit_${countSuffix}`, {
          name,
          count: affectedCount,
        })
      : t(`secureInput.blockedNoCulprit_${countSuffix}`, {
          count: affectedCount,
        });

  const steps =
    name !== null
      ? [
          t("secureInput.fix.leavePasswordFieldWithCulprit", { name }),
          t("secureInput.fix.reopenWithCulprit", { name }),
          t("secureInput.fix.lockScreen"),
        ]
      : [
          t("secureInput.fix.leavePasswordField"),
          t("secureInput.fix.reopen"),
          t("secureInput.fix.lockScreen"),
        ];

  return (
    <div className="w-full rounded-lg border border-warning/40 bg-warning/10 px-3 py-2.5">
      <div className="flex items-center gap-3">
        <TriangleAlert className="h-5 w-5 shrink-0 text-warning" />
        <p className="min-w-0 flex-1 text-sm font-medium leading-5">
          {message}
        </p>
        <div className="flex shrink-0 items-center gap-1">
          <button
            onClick={() => setShowFix((v) => !v)}
            aria-expanded={showFix}
            className="cursor-pointer whitespace-nowrap rounded px-2 py-1.5 text-sm font-medium text-text hover:text-warning focus:outline-none focus:ring-1 focus:ring-warning"
          >
            <span className="flex items-center gap-1 border-b border-current leading-4">
              {t("secureInput.learnMore")}
              <ChevronDown
                className={`h-3.5 w-3.5 transition-transform ${showFix ? "rotate-180" : ""}`}
              />
            </span>
          </button>
          <button
            onClick={() => setDismissed(true)}
            aria-label={t("secureInput.dismiss")}
            className="cursor-pointer rounded p-1.5 text-mid-gray hover:bg-warning/15 hover:text-warning focus:outline-none focus:ring-1 focus:ring-warning"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </div>
      {showFix && (
        <div className="mt-2 pl-8 text-sm leading-5">
          <ol className="list-decimal space-y-1 pl-4">
            {steps.map((step) => (
              <li key={step}>{step}</li>
            ))}
          </ol>
          <p className="mt-2 text-mid-gray">{t("secureInput.fix.noRestart")}</p>
        </div>
      )}
    </div>
  );
};

export default SecureInputWarning;
