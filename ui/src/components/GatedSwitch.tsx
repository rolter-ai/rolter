import { Switch, type SwitchProps } from "@/components/ui/switch";
import { useGate, type Capability } from "@/lib/can";

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
 */
export function GatedSwitch({
  gate,
  disabled,
  title,
  ...props
}: SwitchProps & { gate: Capability }) {
  const { denied, reason } = useGate(gate);
  return (
    <Switch
      {...props}
      disabled={disabled || denied}
      title={denied ? reason : title}
    />
  );
}
