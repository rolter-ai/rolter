import { useTranslation } from "react-i18next";

import { InfoHint } from "@/components/ui/info-hint";
import { Switch } from "@/components/ui/switch";
import { useGate, type Capability } from "@/lib/can";
import { useRefusedClick } from "@/lib/ux-react";

// a boolean as a full-width row: title, optional hint, switch on the right.
// the switch is named by the title, so it is never an unlabelled toggle.
//
// `gate` refuses the switch up front the way `GatedSwitch` is refused (#1183),
// with the role it needs in its title, instead of letting it flip and snap back
// when the 403 lands. a gated one must name itself with `control` so a reach
// for it lands in the UX stream as `refused_click` (#1759)
export type SwitchRowProps = SwitchRowBaseProps &
  (
    | {
        /** the `resource:action` the switch changes */
        gate: Capability;
        /** names this control in the UX stream when it is refused (#1750) */
        control: string;
      }
    | { gate?: undefined; control?: undefined }
  );

interface SwitchRowBaseProps {
  title: string;
  hint?: string;
  info?: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}

export function SwitchRow({
  title,
  hint,
  info,
  checked,
  onChange,
  disabled,
  gate,
  control = "switch",
}: SwitchRowProps) {
  const { t } = useTranslation();
  const { denied, reason } = useGate(gate);
  const refusal = useRefusedClick(denied, control, gate);
  return (
    <div className="flex items-center gap-3 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3.5 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="text-sm">{title}</span>
          {info && <InfoHint text={info} label={t("common.aboutField", { label: title })} />}
        </div>
        {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
      </div>
      {/* `display: contents` for the reason `GatedSwitch` gives: on the event
          path, out of the layout */}
      <span className="contents" {...refusal}>
        <Switch
          checked={checked}
          onCheckedChange={onChange}
          disabled={disabled || denied}
          title={denied ? reason : undefined}
          aria-label={title}
        />
      </span>
    </div>
  );
}
