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

The control that opened the dialog carries `pending` too, scoped to its row. A
list has one mutation for every row, so `pending={remove.isPending}` alone puts a
spinner on every row's control while one delete is out (#2095). Compare the row
with the target, `remove.isPending && target?.id === row.id`, the way `Pricing`
and `Limits` do.

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
  last enabled one and the saved policy has password sign-in off, the control
  plane refuses both with a 409 (#2443), so the card disables the enable switch
  and the delete button instead of confirming, with a reason beside them
  (`pages.sso.lastMethod.reason`) that the controls reference through
  `aria-describedby`. `locksOutMembers` in `ui/src/lib/sso-lockout.ts` decides
  it. It reads the saved policy, not the draft on the policy card, and a
  provider that is already out of service never counts. A 409 that still comes
  back (another admin changed the list first) is shown as
  `pages.sso.lastMethod.refused`, not as the server's raw message.
- **Turning password sign-in off** confirms as `sso-password-off`, and lists the
  enabled providers with no stored client secret (`secretGap`). It warns and
  never blocks, since a public client has no secret on purpose. A save that
  also tightens the second factor raises this dialog first and the second-factor
  one after it, and sends a single request.
- **Turning single sign-on off** confirms as `sso-single-sign-on-off` (#2326)
  when the org has an enabled provider, with a `SsoOffNotice` as `children`.
  `locksOutSsoMembers` in `ui/src/lib/sso-lockout.ts` decides it from the saved
  policy and the draft: only the flip from on to off counts, and with no enabled
  provider (none at all, or every one out of service) nobody signs in through
  one, so the save goes straight out. Turning it on asks nothing. The dialog
  body and notice state only what the control plane enforces: while `allow_sso`
  is off the org's providers drop out of `GET /api/v1/auth/methods` (the
  sign-in screen's buttons disappear), `/auth/sso/{slug}/start` refuses at once
  and the callback refuses a login begun before the switch (#2339, #2605), and an account created through a
  provider has no password, so those members cannot sign in until single sign-on
  is back on or a superadmin sets one. An account that holds a password, such as
  one made from an invitation, keeps signing in, because password sign-in stays
  on (the control plane refuses both off). It warns and never blocks, since one
  flip undoes it. The providers come from the list the screen already read, so a
  caller refused that list gets no confirmation.
- **The second-factor confirmation** counts people with `distinctPeople`: the
  memberships endpoint returns one row per grant, so a person holding a role on
  the org and another on a team is one member.

A save that needs more than one of these asks them in turn, password sign-in
first, then single sign-on, then the second factor, and sends one request after
the last. The steps are listed once in `SignInPolicyCard`, and only the last
confirmation passes `pending`: a confirmation that merely moves on runs no
request, so it must not report a landing.

The control plane refuses the mirror change, turning passwords off with no
enabled provider, but does not refuse these (#2233 tracks that guard). The
dialog stays useful once it lands, as the explanation that comes before the
refusal.

Three account changes on the Users screen raise it too (#2055, #1893). Each
names the account by email and states what the control plane does:

- **Deactivating** confirms as `user-deactivate` with `tone="danger"`. The body
  says sign-in is blocked, every session the account has open ends at once and
  the virtual keys it minted for itself stop working at the gateway, and that
  reactivating brings sign-in and those keys back. Reactivating sends at once,
  with an icon and a label of its own: it only gives access back, and one click
  on deactivate undoes a misfire.
- **Deleting** confirms as `user-delete` with `tone="danger"`, raised from the
  edit sheet, which stays open behind it so a cancel returns to the form. The
  body says the account leaves every organization and not only the one on
  screen, what goes with it (its roles and sessions; the keys it minted for
  itself are disabled, not deleted), and points at deactivating, the reversible
  way to block a person. A landed delete closes the sheet with it.
- **Granting superadmin** confirms when the sheet is saved, since that is when
  the flag is granted: `user-superadmin-grant` with `tone="default"`, because
  it is not a removal and one flip undoes it. The body says what the flag hands
  over, every organization, team and project and every deployment-wide setting
  and account. Turning the flag off saves directly, except on the caller's own
  account (below).

None of the three self-lockouts is refused by the control plane: a superadmin
can deactivate or delete the account they are signed in with, or take superadmin
off it, and the last active superadmin can go the same way (#2344 tracks that
guard). So when the target is the caller's own account
(`useOptionalAuth().user.id`), each dialog adds a sentence saying the caller is
signed out now, or loses the access they are using, and that only another
superadmin can undo it. Taking the flag off one's own account is the one case
where removing superadmin asks (`user-superadmin-remove`, `tone="danger"`).
The screen never claims an account is the last superadmin: its users list holds
only the people with a role in the selected organization, so it cannot know.

`EditUserDialog` stays mounted whether or not a sheet is open, and the
confirmations sit beside the sheet rather than inside it, for the reason the
guardrail one does: a save that lands closes the sheet in the same commit, and
a dialog inside it would unmount before it reported `save_confirmed`. The
sheet's own `user-edit` rows record the press of Save, so a save that only
raised the question still reads as a `form_submit` there; the confirmation's
rows are the ones that say whether anything was sent.

A `Dialog` paints above an editor sheet. The sheet's layer is `z-[80]`, the
dialog's `z-[85]` and the toaster's `z-[90]`. Until #2055 the dialog sat at
`z-50`, under the sheet's own scrim with its action half covered by the panel,
which every confirmation raised over a sheet had inherited, the discard prompt
included. The assertion is a z-index comparison, because the sheet is `inert`
under the dialog and no query tells the layers apart (`expectPaintsOver` in
`Users.stories.tsx`).

Mapping an identity-provider group to a role confirms when the grant reaches
far (#2078), on the SCIM and the single sign-on screens alike. Both render the
shared `GroupMappings` (`ui/src/components/GroupMappings.tsx`), so the rule is
written once:

- **The form starts on `viewer`**, chosen by name in the component rather than
  read off `ROLES`, whose order other screens depend on. After a mapping is
  written it starts over, so the next one does not inherit an admin grant.
- **`admin`, or any role at the whole organization, confirms** as
  `<kind>-group-mapping-grant` (`scim` or `sso`) with `tone="default"`, since
  it is a grant and not a removal. The scope starts on the whole organization,
  so a mapping left at its defaults confirms. A `member` or `viewer` mapping on
  one team or one project is sent at once.
- **The dialog names which of the two raised it.** The title carries the group
  and the role, the body the scope and, from the screen, when the role
  arrives (SCIM reconciles on the spot, single sign-on at the member's next
  sign-in), and `children` lists the reasons: what `admin` is, and that the
  whole organization is every team and project rather than one. Both reasons
  appear for `admin` across the organization.
- **A refusal stays in the dialog.** The mutation is reset on cancel, and the
  form's own inline error is not drawn while the dialog is up, so the message
  appears once.

Removing a mapping keeps its own confirmation, `<kind>-group-mapping-remove`,
whose body each screen supplies because what removal does differs: SCIM
withdraws the role at once, single sign-on stops granting it at the next
sign-in.

A save on the Security screen that loosens the gateway or the dashboard raises the same dialog
(#2103), as `security-loosen` with `tone="default"`, since one more save undoes it. Three edits
loosen and nothing else does: virtual-key enforcement turned off, dashboard protection turned off,
and each route added to the auth bypass list. `loosenings` in `ui/src/lib/security-loosening.ts`
decides, comparing the draft with what the store held at the last load or save rather than with the
previous keystroke, so a switch flipped off and back on asks nothing. The body lists exactly the
changes that opened something, each with what it means, as `children`, and the title and intro
count them. A tightening, such as a route removed or a header required, saves at once, and one
request goes out either way. The dialog is mounted beside the form, and the items it lists are kept
after it closes so its body does not empty while it fades. The words state the documented meaning of
each setting and no more: see
[the Security screen](../architecture/security.md#the-security-screen-2103-2114).

Deleting a provider names what still points at it (#2143). The Providers screen reads the
effective config and the org's provider groups when the confirm opens, not on every visit, and
hands the answer to `ProviderUsageNotice` as `children`. `providerUsage` in
`ui/src/lib/provider-usage.ts` matches routes by provider name, which is unique across the
deployment, and groups by member id, and marks a route or group the provider is the whole of. The
notice never guesses: while either read is out it holds the space with a `LoadingRegion`, and a
failed one is a `LoadError` with a retry, so neither reads as "nothing uses it". The confirm stays
pressable in every state, since the control plane has the last word: `route_targets` and
`provider_group_members` reference `providers` with `on delete restrict`, so it refuses a provider
that is still referenced. The body says what a delete does to a client addressing the provider
directly as `provider-slug/model`, which holds whether or not a route exists.

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

**An inline two-step panel is not a confirmation either.** The Users sheet
deleted through a destructive button that flipped a local flag and swapped in
its own body, error line and second button (#1893). No `DialogFooter` held it,
so `check:primitives` never saw it, and it reported none of the rows below.
#2345 tracks teaching the check that shape.

**No confirmation at all is the case no check catches.** `DeleteIconButton`
looks the same whether its `onClick` opens a dialog or sends the request, so
`check:primitives` has nothing to match, and the budget and rate-limit deletes on
`Limits` fired on the first click long after #1179 swept the other screens
(#1904). What pins it is the story: its cancel step looks for a dialog to
dismiss, and a delete that never asked has none.

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

## Closing over a one-time secret

A virtual key, a SCIM token and an invitation link are shown once and stored as
a digest or not at all, so closing the dialog that shows one is the point where
it is lost. The shared reveal asks before it closes over a value nobody copied,
through `ConfirmDialog` with `tone="default"`, and asks nothing once the value
has reached the clipboard. The guard, the failed-copy message and the next step
are described in [Dashboard one-time secrets](secret-reveal.md).

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
