import { useTranslation } from "react-i18next";

import { InfoHint } from "@/components/ui/info-hint";

// the compact label a sheet's form rows carry: smaller and quieter than the
// `Field` label, because a sheet stacks dozens of them in one scroll. `Field`
// owns the label+control+error assembly for a screen; this names one control
// in a hand-laid-out row where the assembly is already there (#1044)
export interface FieldLabelProps {
  label: string;
  required?: boolean;
  info?: string;
  /** the control this names, so no label dangles */
  htmlFor?: string;
  /** for a group (a segmented control) that is `aria-labelledby` this id */
  id?: string;
}

export function FieldLabel({ label, required, info, htmlFor, id }: FieldLabelProps) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-1.5">
      <label
        id={id}
        htmlFor={htmlFor}
        className="text-xs font-medium text-[color:var(--text-secondary)]"
      >
        {label}
      </label>
      {required && <span className="text-xs text-[color:var(--status-danger-text)]">*</span>}
      {info && <InfoHint text={info} label={t("common.aboutField", { label })} />}
    </div>
  );
}
