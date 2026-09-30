// The words the dashboard puts on the states alerting stores as enums (#2126).
//
// The API hands back bare identifiers, and each one used to be printed as it
// came, so a Russian screen read "FIRING" and "SKIPPED". They are the ones
// `crates/rolter-control/src/alerting.rs` writes:
//
//   - a rule is `unknown` (never evaluated), `ok`, `firing` or `error` (its last
//     evaluation failed);
//   - a history row records the state a rule moved to, `firing` or `resolved`,
//     and whether telling the channel worked: `delivered`, `failed` or
//     `skipped`;
//   - a channel has a `kind`, which is `webhook` today.
//
// A value this build does not know prints as stored rather than as a blank or a
// raw catalog key, the way `signalLabel` treats a signal from a later build.

type Translate = (key: string, options?: Record<string, unknown>) => string;

export const RULE_STATES = ["unknown", "ok", "firing", "error"] as const;
export const HISTORY_STATES = ["firing", "resolved"] as const;
export const DELIVERY_STATUSES = ["delivered", "failed", "skipped"] as const;
export const CHANNEL_KINDS = ["webhook"] as const;

// the catalog names a state once, whichever list it came from
const STATE_KEYS = ["unknown", "ok", "firing", "error", "resolved"] as const;

const isOneOf = (list: readonly string[], value: string) => list.includes(value);

/** a rule's or a history row's state, translated; an unknown one as stored */
export function stateLabel(state: string, t: Translate): string {
  return isOneOf(STATE_KEYS, state) ? t(`pages.alerting.states.${state}`) : state;
}

/** whether a delivery reached its channel, translated; an unknown one as stored */
export function deliveryLabel(status: string, t: Translate): string {
  return isOneOf(DELIVERY_STATUSES, status) ? t(`pages.alerting.deliveries.${status}`) : status;
}

/** what kind of destination a channel is, translated; an unknown one as stored */
export function channelKindLabel(kind: string, t: Translate): string {
  return isOneOf(CHANNEL_KINDS, kind) ? t(`pages.alerting.channels.kinds.${kind}`) : kind;
}
