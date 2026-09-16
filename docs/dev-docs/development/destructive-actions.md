# Dashboard destructive actions

A control that cannot be undone has to ask first, and it has to say what it is
about to destroy. Before #1179 the dashboard did this three different ways at
once: eight screens confirmed through a hand-rolled `Dialog`, nine deleted on a
single click with nothing between the pointer and the request, and four fell
back to `window.confirm`.

That is not only inconsistent, it is inconsistent in the direction that costs
the most. `Account` rotated a virtual key on one click — the old secret stops
authenticating immediately, so a stray click breaks every client using it, with
no undo and no warning. `Alerting` deleted the channel every rule delivered
through. `Cluster` forgot a node's history. None of them named the row they were
about to act on.

## The rule

Every destructive action goes through `ConfirmDialog`
(`ui/src/components/ConfirmDialog.tsx`):

```tsx
const [target, setTarget] = React.useState<ChannelRow | null>(null);
// reset first: an error from a previous failed delete would otherwise greet
// the next row the operator picks
const startDelete = (channel: ChannelRow) => {
  remove.reset();
  setTarget(channel);
};

<ConfirmDialog
  open={!!target}
  onOpenChange={(open) => !open && setTarget(null)}
  title={t("pages.alerting.confirm.channelTitle", { name: target?.name })}
  description={t("pages.alerting.confirm.channelBody")}
  confirmLabel={t("pages.alerting.confirm.channelConfirm")}
  pending={remove.isPending}
  error={remove.error}
  onConfirm={() =>
    target && remove.mutate(target.id, { onSuccess: () => setTarget(null) })
  }
/>
```

Four properties are load-bearing:

- **The title names the thing.** "Delete channel ops-slack?", never "Are you
  sure?". A confirmation that could be about anything is a click-through, and a
  click-through is worse than no dialog at all — it trains the habit that gets
  the wrong row deleted.
- **The description states the consequence in one sentence.** What stops
  working, and whether anything else goes with it. `McpOAuth` is the model:
  revoking a *grant* cascades to its sessions and the dialog counts them;
  revoking a *session* does not, and the dialog says so.
- **The dialog does not close itself on confirm.** The caller closes it from
  `onSuccess`. A mutation that fails leaves the dialog open with the control
  plane's own message on a `role="alert"` line, because closing would drop the
  only place the failure could be reported.
- **Pending is visible and both buttons are out of reach.** The confirm button
  spins so a slow request does not read as a dropped click, and cancel is
  disabled too — the request is already on the wire, and a button that looks
  like it recalls one would be lying.

`tone` picks the confirm button's paint: `danger` (the default) for deletions
and revocations, `default` for something irreversible that is not a removal —
key rotation is the case that motivated it.

## What this is not

**`window.confirm` is not an option.** It cannot be styled, cannot be
translated — so it is invisible to `check:i18n` and to the locale catalogs
entirely — and in the story runner it is a modal nothing can answer, which means
the confirm path of four screens had never been exercised by a test. `rg
"window.confirm" ui/src` should only ever find the sheets' discard guards, which
have a different dismiss contract (they answer "may I throw this draft away",
not "may I destroy this row") and are tracked separately.

**A confirmation is not a substitute for a reversible action.** Where retiring
and deleting both exist — `CostAttribution` — the copy points at the reversible
one rather than only warning about the other.

## The control names its row too

The dialog naming the row is only half of it. The control that opens it needs
the same name, because a list of twelve rows otherwise exposes twelve buttons
whose accessible name is the identical `"Delete channel"` — a screen reader
user tabbing the grid hears it twelve times with nothing to tell them apart,
and the row's identity lives only in the visual layout (#1214).

So the `aria-label` carries the row: `t("pages.alerting.channels.deleteAria",
{ name })`, never a bare `"Delete channel"` and never a template literal —
`check:literals` matches quoted strings only, so an interpolated one would stay
untranslated with the gate still green. The copy lives under
`pages.<screen>.*Aria` in every catalog, like all the rest.

The payoff is in the tests: a story selects its delete button by that name
rather than by index, so it asserts on the control it means instead of trusting
the confirmation dialog to prove the right row was picked afterwards.

## Copy and stories

Strings live under `pages.<screen>.confirm.*` in **every** catalog under
`ui/src/lib/i18n/locales/` (see [i18n](i18n.md)); the title carries the item's
name as an interpolation, so the placeholder has to survive translation.

Each screen's story clicks the destructive control, asserts the dialog names the
row, cancels once to prove nothing was sent, then confirms and asserts the
request left. `ui/src/pages/story-harness.tsx` supplies `recording`,
`confirmDestructive` and `cancelConfirmation` for exactly this shape.
