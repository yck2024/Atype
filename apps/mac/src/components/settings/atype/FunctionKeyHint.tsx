import React from "react";
import { useTranslation } from "react-i18next";
import { useSettings } from "../../../hooks/useSettings";
import { useOsType } from "../../../hooks/useOsType";

/** Matches a binding whose main key is F1–F20, e.g. "f1" or "shift+f5". */
const FUNCTION_KEY = /(^|\+)f([1-9]|1[0-9]|20)$/i;

/**
 * Shown under a shortcut that uses a function key on macOS: by default the
 * top row sends brightness, volume, etc., so F1 never reaches Atype unless
 * fn is held or the keys are set as standard function keys.
 */
export const FunctionKeyHint: React.FC<{ shortcutId: string }> = ({
  shortcutId,
}) => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const osType = useOsType();
  const binding = getSetting("bindings")?.[shortcutId]?.current_binding ?? "";

  if (osType !== "macos" || !FUNCTION_KEY.test(binding)) return null;

  return (
    <p className="px-4 pb-2 -mt-1 text-xs text-mid-gray">
      {t("settings.atype.functionKeyHint")}
    </p>
  );
};
