import type { TFunction } from "i18next";

export function shortcutErrorMessage(
  t: TFunction,
  error: unknown,
  fallback: "set" | "reset",
): string {
  const message = error instanceof Error ? error.message : String(error);
  const prefix = "shortcut-conflict:";
  return message.startsWith(prefix)
    ? t("settings.general.shortcut.errors.conflict", {
        binding: message.slice(prefix.length),
      })
    : t(`settings.general.shortcut.errors.${fallback}`, {
        error: String(error),
      });
}
