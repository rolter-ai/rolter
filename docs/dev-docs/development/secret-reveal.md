# Dashboard one-time secrets

Three values are on screen exactly once: a virtual key, a SCIM bearer token and
an invitation link. The control plane keeps a digest of the first two and nothing
of the third, so the dialog that shows the value is the last time it exists, and
"I pressed Copy and it worked" has to be true before that dialog is closed.

Each screen used to lay the reveal out by hand (#2217). All three copied
silently into a clipboard that a plain-http dashboard never has, closed on
Escape with the value uncopied, and ended at **Done** with nothing said about
what to do with the value. The primitive is `ui/src/components/ui/secret-reveal.tsx`,
and its stories are under **Overlays/SecretRevealDialog**.

## The three parts

| Export                | What it is                                                                                      |
| --------------------- | ----------------------------------------------------------------------------------------------- |
| `SecretValue`         | the value in mono, selectable in one click, its copy button and what a failed copy says         |
| `useSecretCloseGuard` | the question asked before an uncopied value goes, in the shape of `useDiscardGuard`             |
| `SecretRevealDialog`  | a `Dialog` made of the two, with a `children` slot under the value for the step that comes next |

A reveal inside a sheet takes the body and the guard without the dialog shell.
The SCIM token is the case: the base URL the connector needs sits beside the
token, in the same sheet, so `IssueTokenSheet` renders `SecretValue`, passes
`guard` to `Sheet.onDismiss` and `close` to the header button and **Done**, and
mounts `prompt` among the sheet's children.

```tsx
<SecretRevealDialog
  name="virtual-key-created"
  open={!!created}
  onOpenChange={(open) => !open && setCreated(null)}
  title={t("pages.virtualKeys.createdTitle")}
  description={t("pages.virtualKeys.createdBody")}
  secret={created?.key ?? ""}
  copyLabel={t("common.copy")}
  size="lg"
>
  <KeyNextStep models={created?.models ?? []} />
</SecretRevealDialog>
```

Drive `open` from the secret's own state, so the value is dropped when the
dialog closes and can never be read back. `name` is the stable key for the UX
stream, never the value; the close question reports as `<name>-close`.

## A failed copy stays on screen

`CopyButton` reports a failure with a tooltip, a live region and a red glyph
that resets after 1.6 seconds. That is enough for an address that can be copied
again and too little for a value that cannot (#2327). Two opt-in props on
`CopyButton` carry the difference, and a call site that passes neither behaves as
before:

- `persistFailure` holds the failed state until the next press or until `value`
  changes, instead of resetting on a timer.
- `onStateChange` is told every state the button moves to, including a second
  failure on the same value, so a caller can draw a message next to the value,
  where an icon button has no room for one.

`SecretValue` uses both. A failure leaves a `role="alert"` line under the value
(`common.copyFailed`, in the danger text token) that says to select the value
and copy it by hand, with a **Select value** button. The value is also selected
as soon as the copy fails, so copying it by hand is one keystroke.

The value is `select-all`, which is what "selectable in one gesture" means. A
copy made by hand counts: `SecretValue` listens for the document's `copy` event
and treats a selection that holds the whole value as copied, so a person who
selected it and pressed Ctrl+C is not then asked whether they copied it. A
partial selection does not count.

## Closing asks while the value is uncopied

Escape, the scrim, the close button and **Done** all arrive at one guard.
Uncopied, they raise a `ConfirmDialog` ("Close without copying? It will not be
shown again.") with `tone="default"`, since closing is the question and not a
removal. Cancelling keeps the reveal and the value on screen; only the explicit
confirm closes it. Once the value has reached the clipboard, by the button or by
hand, closing asks nothing, because a prompt that always appears is one people
learn to click through. See [destructive actions](destructive-actions.md) for
`ConfirmDialog` and [dismissing a dirty editor](destructive-actions.md#dismissing-a-dirty-editor)
for the guard this one mirrors.

Stories answer the question with `answerSecretClosePrompt(true | false)` in
`ui/src/pages/story-harness.tsx`, which finds it by its accessible name, since
the reveal is still mounted behind it. `stubClipboard(writeText)` stands in for
the clipboard, and a `writeText` that rejects is the plain-http case.

## The next step

A key is revealed so it can be used, and the dialog used to end at **Done**.
`KeyNextStep` (`ui/src/components/KeyNextStep.tsx`) sits in the `children` slot
of both key reveals, on **Governance → Virtual Keys** and **My Virtual Keys**:

- the gateway address, from `useGatewayBase()`, so it is the public base URL
  saved on Client Settings when the caller may read it and the dashboard's `/gw`
  proxy otherwise, with `/v1` on it the way the OpenAI SDKs take it (#2218)
- a curl request from `renderSnippet`, the builder the Playground's copy-as-code
  uses, so there is one snippet builder and one gateway address. The model is
  the first the key may reach, or the built-in `fake-llm` for a key that may
  reach every route
- the key is referenced as `$ROLTER_API_KEY` and never written into the snippet,
  because a request pasted into a ticket or a chat must not carry a working
  credential

The SCIM reveal's next step is the SCIM base URL beside the token, which it
already showed. The invitation link's is who to send it to and what accepting it
grants, passed as `children`. Those dialogs are narrow, since they hold no
snippet. A dialog that carries one takes `size="lg"`: a snippet is a document,
and a line broken mid-token reads worse than a wider panel (#948).
