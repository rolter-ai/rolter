// the words the dashboard puts on a provider's `kind` (#2839).
//
// the control plane stores and sends a bare identifier (`openai_compatible`),
// and the catalog names each one under `providerSheet.kinds.<kind>`. a kind a
// later control plane gained and this catalog has not named prints as stored,
// the way the provider sheet's picker always did, so a row never goes blank or
// shows a raw catalog key

import type { TFunction } from "i18next";

/** a kind's display name, or the stored id where the catalog does not name it */
export function providerKindName(kind: string, t: TFunction): string {
  return t(`providerSheet.kinds.${kind}.name`, { defaultValue: kind });
}

/** a kind's one-line description, or "" where the catalog has none */
export function providerKindDescription(kind: string, t: TFunction): string {
  return t(`providerSheet.kinds.${kind}.description`, { defaultValue: "" });
}
