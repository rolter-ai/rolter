import { Combobox, type ComboboxProps } from "@/components/ui/combobox";
import { useGate, type Capability } from "@/lib/can";
import { useRefusedClick } from "@/lib/ux-react";

/**
 * The `Combobox` a row setting uses, refused the way a switch is (#1759).
 *
 * A picker on a row — a key's cache mode — is the same `<resource>:update` its
 * toggle and its edit sheet need. Keys used to disable it from its own
 * `useGate`, which disabled it silently: the reach for it never reached the UX
 * stream. `control` names it there, and is required for the reason it is on
 * `GatedButton` (#1750).
 */
export function GatedCombobox({
  gate,
  control = "combobox",
  disabled,
  title,
  ...props
}: ComboboxProps & { gate: Capability; control: string }) {
  const { denied, reason } = useGate(gate);
  const refusal = useRefusedClick(denied, control, gate);
  return (
    <span className="contents" {...refusal}>
      <Combobox {...props} disabled={disabled || denied} title={denied ? reason : title} />
    </span>
  );
}
