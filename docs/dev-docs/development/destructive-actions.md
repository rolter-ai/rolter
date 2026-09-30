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

<ConfirmDialog
  // stable key for the UX stream; never the row's own name (#1730)
  name="alert-channel-delete"
  open={!!target}
  onOpenChange={(open) => {
    if (open) return;
    setTarget(null);
    // an error from a failed delete would otherwise greet the next row opened
    remove.reset();
  }}
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

Five properties are load-bearing:

- **The title names the thing.** "Delete channel ops-slack?", never "Are you
  sure?". A confirmation that could be about anything is a click-through, and a
  click-through is worse than no dialog at all — it trains the habit that gets
  the wrong row deleted.
- **The description states the consequence in one sentence.** What stops
  working, and whether anything else goes with it. `McpOAuth` is the model:
  revoking a _grant_ cascades to its sessions and the dialog counts them;
  revoking a _session_ does not, and the dialog says so.
- **The dialog does not close itself on confirm.** The caller closes it from
  `onSuccess`. A mutation that fails leaves the dialog open with the control
  plane's own message on a `role="alert"` line, because closing would drop the
  only place the failure could be reported.
- **Pending is visible and both buttons are out of reach.** The confirm button
  spins so a slow request does not read as a dropped click, and cancel is
  disabled too — the request is already on the wire, and a button that looks
  like it recalls one would be lying. Escape, the scrim and the header's close
  button still close the dialog: neither the fetch nor the control plane times
  a request out, so a delete stuck behind a row lock would otherwise hold the
  operator in a full-page modal until a reload.
- **Focus stays in the dialog while it is busy.** Disabling the button that was
  just pressed makes the browser drop focus onto `<body>`, outside the panel
  whose Tab trap listens for the next key. `useModalA11y` hands focus back to
  the panel, and keeps everything outside the topmost modal `inert` while it is
  up, so a Tab has nothing behind the scrim to land on (#1998).

`tone` picks the confirm button's paint: `danger` (the default) for deletions
and revocations, `default` for something irreversible that is not a removal —
key rotation is the case that motivated it.

A save that hands over control of every request goes through the same dialog,
with `tone="default"`. Activating a guardrail provider pauses the active one
and sends all traffic through the new one, so `GuardrailProviders` raises the
confirmation over its provider dialog. The confirmation names the provider it
pauses and says what the failure policy then does to a request (#2163). The
provider dialog stays open behind it, so a cancel returns to the form with the
fields intact. The confirmation is mounted beside that dialog rather than
inside it: the dialog remounts as it closes, and a confirmation inside it would
unmount before it could report `save_confirmed`.

Switching the active provider off is the mirror case and goes through the same
confirmation (#2271). It names the provider and says no external guardrail
checks requests afterwards, with `tone="danger"`, or that the config-file
webhook stays in force, with `tone="default"`. It reports under
`guardrail-provider-pause`. The screen keeps which of the two it raised after
the dialog closes: the landing is reported on the closing edge, and a key that
flipped back to the activation's there would file the pause under it.

A publish that changes which prompt reaches live traffic takes `default` as
well, and its confirmation says who the change will refuse. Publishing or
rolling back a prompt template version (`MakeLiveDialog` in
`PromptRepository.tsx`, #2110) lists, as its `children`, the scopes the version
reaches, the variables a request there must send and the ones the live version
declared that this one drops, computed from the gateway's own rules in
`ui/src/lib/prompt-templates.ts`. It warns and never sets `confirmDisabled`,
since the same dialog is how an operator rolls back mid-incident. Its `name` and
direction are latched when it opens: the landing is reported on the render that
closes it, and by then the version is the live one, so a direction read again
would file a publish as a roll back.

A change to the sign-in policy raises the same dialog when it can shut members
out (#2084), on the Single Sign-On screen:

- **Taking a provider out of service** confirms as `sso-provider-disable` with
  `tone="default"`, since one flip undoes it. Switching a provider back on sends
  at once. Deleting one keeps `sso-connection-delete`. When the provider is the
  last enabled one and the saved policy has password sign-in off, both carry a
  `LockoutNotice` as `children` and the disable button turns `danger`.
  `locksOutMembers` in `ui/src/lib/sso-lockout.ts` decides it. It reads the
  saved policy, not the draft on the policy card, and a provider that is already
  out of service never counts. The notice states only what the control plane
  enforces: superadmins are exempt from `allow_password_login = false`, and an
  account created through a provider has no password, so turning password
  sign-in back on does not restore it.
- **Turning password sign-in off** confirms as `sso-password-off`, and lists the
  enabled providers with no stored client secret (`secretGap`). It warns and
  never blocks, since a public client has no secret on purpose. A save that
  also tightens the second factor raises this dialog first and the second-factor
  one after it, and sends a single request.
- **The second-factor confirmation** counts people with `distinctPeople`: the
  memberships endpoint returns one row per grant, so a person holding a role on
  the org and another on a team is one member.

The control plane refuses the mirror change, turning passwords off with no
enabled provider, but does not refuse these (#2233 tracks that guard). The
dialog stays useful once it lands, as the explanation that comes before the
refusal.

## What this is not

**`window.confirm` is not an option.** It cannot be styled, cannot be
translated — so it is invisible to `check:i18n` and to the locale catalogs
entirely — and in the story runner it is a modal nothing can answer, which means
the confirm path of four screens had never been exercised by a test. `rg
"window.confirm" ui/src` now finds nothing outside the `check:literals` fixtures:
the discard guards #1179 left behind came over in #1463, described below.

**A hand-assembled `Dialog` is not a confirmation either.** Eight dialogs
drifted back into a `DialogFooter` with their own destructive `Button` after
#1179: `ProviderGroups`, `Models`, `Pricing`, `PromptRepository`,
`SkillsRepository`, the MCP server delete, the `Account` key delete and the
scope switcher (#1760). Each worked out pending, error and disabled on its own,
emitted none of the rows below, and missed every fix made to `ConfirmDialog`.
`bun run check:primitives` now fails on a `DialogFooter` holding a
`"destructive"` button; `ConfirmDialog` itself carries the only waiver. A
confirmation that needs input, such as the prompt and skill deletes that ask
for the slug typed back, passes the field as `children` and holds the button
with `confirmDisabled`.

**A confirmation is not a substitute for a reversible action.** Where retiring
and deleting both exist — `CostAttribution` — the copy points at the reversible
one rather than only warning about the other.

## What the dialog reports

`ConfirmDialog` feeds the UX stream through `useFormTelemetry` under its `name`
(see [UX telemetry](ux-telemetry.md)). The caller owns the mutation, so the
dialog reads the outcome off the props that report it:

| Row                      | When                                                               |
| ------------------------ | ------------------------------------------------------------------ |
| `form_submit` `ok`       | the confirm button is pressed                                      |
| `form_submit` `error`    | an `error` arrives after the press that was not already on screen  |
| `retry_submit`           | the confirm is pressed again after a refusal                       |
| `save_confirmed`         | the caller closes the dialog after the press, with nothing refused |
| `form_abandon` cancelled | the dialog closes without a press                                  |

The refusal is read off the press rather than off a `pending` edge (#1761). A
request that settles in the tick it started hands react-query's pending and
error to one notify batch, so `pending={true}` never renders, and an
edge-triggered read reported nothing but the press. It is keyed on the error's
identity, so a retry refused the same way is a row of its own.

`save_confirmed` is how a landed delete looks from inside the dialog: the
caller closes it from `onSuccess`, so closed after a press with no new error is
the success. Passing `pending` at all is what marks a confirmation that runs a
request. The discard prompt passes none, closes the sheet and is done, so it
reports no landing.

Two things follow for a call site:

- **Keep the dialog mounted** and drive it with `open={!!target}`. A dialog
  rendered only while a target exists unmounts on the closing edge and never
  sees the landing.
- **Reset the mutation on close** (`remove.reset()` in `onOpenChange`), so a
  refusal from one row does not greet the next row opened. A dismissal while
  `pending` resets a mutation whose answer is still coming, which from inside
  the dialog looks exactly like a landing, so the dialog disarms its read
  before it passes the dismissal on: the press is recorded and nothing after
  it.

A confirmation rendered outside any `UxScreenProvider`, such as the scope
switcher in the user menu, has no screen key and stays silent.

## Dismissing a dirty editor

The editor sheets ask a different question — "may I throw this draft away",
not "may I destroy this row" — but they ask it the same way, through
`useDiscardGuard` (`ui/src/components/DiscardGuard.tsx`). `EditorSheet` wires it
for every screen built on the shell; `ModelSheet`, `ProviderSheet` and
`ProviderGroupSheet` call it directly because they assemble their own chrome.

```tsx
const { guard, close, locked, prompt } = useDiscardGuard({
  dirty,
  saving: save.isPending,
  onOpenChange,
});

<Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
  <SheetHeader title={title} subtitle={subtitle} onClose={close} closeDisabled={locked} />
  ...
  <Button variant="ghost" disabled={locked} onClick={close}>{t("common.cancel")}</Button>
  {prompt}
</Sheet>
```

`Sheet.onDismiss` answers synchronously — it was shaped around the browser
prompt — so the hook keeps that contract by _refusing_ the dismissal and raising
the dialog, then closing the sheet from the dialog's confirm one tick later. The
four dismissal paths (Escape, the scrim, the header's close button, Cancel) all
land on it, and the rules are:

- **A pristine editor closes without asking.** A prompt on a form nobody edited
  is what trains people to click through the one that matters.
- **Cancelling keeps everything.** The draft is the caller's state and is never
  touched; `useModalA11y` returns focus to the control that raised the prompt.
  Only a confirmed discard closes the sheet, and the draft is re-seeded on the
  next open.
- **One prompt, never a queue.** Raising it is idempotent, so Escape twice, or
  Escape then a scrim click, is still one dialog. The browser prompt serialised
  for free; a rendered one has to say so.
- **A save in flight refuses dismissal outright.** The request is already on the
  wire and nothing here can call it back, so a sheet that vanished would leave
  the operator unable to tell whether the mutation landed. Cancel and the close
  button are disabled while `locked` rather than silently no-opping, and focus
  stays inside the sheet the same way it does in a busy confirmation.

Stories answer the prompt through `answerDiscardPrompt(true | false)` in
`story-harness.tsx`, which finds it by its accessible name — the sheet is still
mounted behind it, so `sheet()` cannot tell the two `role="dialog"` nodes apart.
`expectClosesWithoutPrompting()` covers the pristine case.

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
