import React from "react";
import { useTranslation } from "react-i18next";

export const FunctionKeyHint: React.FC = () => {
  const { t } = useTranslation();

  return (
    <p className="px-4 pb-2 -mt-1 text-xs text-mid-gray">
      {t("settings.atype.functionKeyHint")}
    </p>
  );
};
