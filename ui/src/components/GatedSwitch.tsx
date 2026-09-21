import { Switch, type SwitchProps } from "@/components/ui/switch";
import { useGate, type Capability } from "@/lib/can";
import { useRefusedClick } from "@/lib/ux-react";

/**
 * The `Switch` a row toggle uses, refused the same way a button is (#1258).
 *
 * A toggle is an update: flipping "enabled" on a provider, a key or an alert
 * rule is the same `<resource>:update` the edit sheet needs, and leaving it
 * live for a viewer means the row visibly moves and then snaps back when the
 * 403 lands. Disabling it up front is the honest version of that.
 *
 * The track carries no text, so the `title` is the only thing that can say
 * why — and unlike `Button`, the switch's disabled styling does not set
 * `pointer-events-none`, so the native tooltip survives without help.
 *
 * `control` names the toggle in the UX stream when it is refused (#1731) —
 * required for the same reason it is on `GatedButton` (#1750).
 */
export function GatedSwitch({
  gate,
  control = "switch",
  disabled,
  title,
  ...props
}: SwitchProps & { gate: Capability; control: string }) {
  const { denied, reason } = useGate(gate);
  const refusal = useRefusedClick(denied, control, gate);
  return (
    <span className="contents" {...refusal}>
      <Switch {...props} disabled={disabled || denied} title={denied ? reason : title} />
    </span>
  );
}
