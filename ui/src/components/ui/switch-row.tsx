import { useTranslation } from "react-i18next";

import { InfoHint } from "@/components/ui/info-hint";
import { Switch } from "@/components/ui/switch";

// a boolean as a full-width row: title, optional hint, switch on the right.
// the switch is named by the title, so it is never an unlabelled toggle
export interface SwitchRowProps {
  title: string;
  hint?: string;
  info?: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}

export function SwitchRow({ title, hint, info, checked, onChange, disabled }: SwitchRowProps) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-3 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3.5 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="text-sm">{title}</span>
          {info && <InfoHint text={info} label={t("common.aboutField", { label: title })} />}
        </div>
        {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
      </div>
      <Switch checked={checked} onCheckedChange={onChange} disabled={disabled} aria-label={title} />
    </div>
  );
}
