import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { EditorSheet } from "./EditorSheet";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { UxScreenProvider } from "@/lib/ux-react";
import {
  answerDiscardPrompt,
  discardPrompt,
  expectClosesWithoutPrompting,
  expectNoUxEvent,
  expectSheetClosed,
  expectUxEvent,
  recordUxEvents,
  sheet,
  uxEvents,
} from "@/pages/story-harness";

/**
 * The shell around a caller-owned draft.
 *
 * `EditorSheet` owns the chrome and the discard guard only, so a story has to
 * bring a form and a dirty flag with it — this is the smallest thing that
 * behaves like the sheets that ship (ModelSheet, ProviderSheet,
 * ProviderGroupSheet all reduce to this).
 */
function Editor({
  seed = "openai-prod",
  saving = false,
  errorMessage,
  canSave = true,
}: {
  seed?: string;
  saving?: boolean;
  errorMessage?: string;
  canSave?: boolean;
}) {
  const [open, setOpen] = React.useState(true);
  const [name, setName] = React.useState(seed);
  const [saved, setSaved] = React.useState<string | null>(null);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open editor
      </button>
      {saved != null && <p>Saved {saved}</p>}
      <EditorSheet
        name="provider-edit"
        open={open}
        onOpenChange={setOpen}
        title="Edit provider"
        subtitle="openai-prod · openai"
        dirty={name !== seed}
        errorMessage={errorMessage}
        saveLabel="Save provider"
        canSave={canSave}
        saving={saving}
        onSave={() => setSaved(name)}
      >
        <Field label="Name">
          <Input value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
      </EditorSheet>
    </>
  );
}

const meta = {
  title: "Overlays/EditorSheet",
  component: EditorSheet,
  parameters: { layout: "fullscreen" },
  // the stories drive the sheet through `Editor`, which owns the props; these
  // satisfy the required-prop contract for the docs page
  args: {
    name: "provider-edit",
    open: true,
    onOpenChange: () => {},
    title: "Edit provider",
    subtitle: "openai-prod · openai",
    dirty: false,
    saveLabel: "Save provider",
    canSave: true,
    saving: false,
    onSave: () => {},
    children: null,
  },
  // every story starts from an empty UX queue and leaves one behind (#1730)
  beforeEach: recordUxEvents,
} satisfies Meta<typeof EditorSheet>;

export default meta;
type Story = StoryObj<typeof meta>;

// the sheet portals onto document.body, so the story's canvas is empty
const screen = () => within(document.body);

export const Default: Story = {
  render: () => <Editor />,
  play: async () => {
    const dialog = within(sheet());
    await expect(dialog.getByText("Edit provider")).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Save provider" })).toBeEnabled();
  },
};

/** Saving: the primary action goes busy so it cannot be pressed twice. */
export const Saving: Story = {
  render: () => <Editor saving />,
  play: async () => {
    const dialog = within(sheet());
    await expect(dialog.getByRole("button", { name: "Save provider" })).toBeDisabled();
  },
};

/** Nothing valid to save yet — the button says so before the round trip does. */
export const CannotSave: Story = {
  render: () => <Editor canSave={false} />,
  play: async () => {
    const dialog = within(sheet());
    await expect(dialog.getByRole("button", { name: "Save provider" })).toBeDisabled();
  },
};

/** A rejected save keeps the draft on screen with the reason beside it. */
export const SaveFailed: Story = {
  render: () => (
    <Editor errorMessage="a provider with slug 'openai-prod' already exists in this org" />
  ),
  play: async () => {
    const dialog = within(sheet());
    await expect(dialog.getByText(/already exists in this org/)).toBeVisible();
    // the form is still there to correct, not replaced by the error
    await expect(dialog.getByLabelText("Name")).toBeVisible();
  },
};

export const Saves: Story = {
  render: () => <Editor />,
  play: async ({ canvasElement }) => {
    const dialog = within(sheet());
    await userEvent.clear(dialog.getByLabelText("Name"));
    await userEvent.type(dialog.getByLabelText("Name"), "openai-eu");
    await userEvent.click(dialog.getByRole("button", { name: "Save provider" }));
    await waitFor(() => expect(within(canvasElement).getByText("Saved openai-eu")).toBeVisible());
  },
};

/**
 * An untouched draft closes without a prompt. A confirm on a form nobody
 * edited is what trains people to click through the one that matters (#868).
 */
export const ClosesCleanWithoutPrompting: Story = {
  render: () => <Editor />,
  play: async () => {
    await expectClosesWithoutPrompting();
  },
};

/**
 * A dirty draft asks first — through the product dialog, not `window.confirm`
 * (#1463) — and "cancel" means the sheet, the draft and the focus all stay put.
 */
export const DiscardGuardKeepsTheDraft: Story = {
  render: () => <Editor />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.type(dialog.getByLabelText("Name"), "-eu");
    const cancel = dialog.getByRole("button", { name: "Cancel" });
    await userEvent.click(cancel);

    // the prompt is the dashboard's own dialog, and it says what is lost
    const prompt = await discardPrompt();
    await expect(
      within(prompt).getByText("The edits in this form have not been saved and will be lost."),
    ).toBeVisible();

    await answerDiscardPrompt(false);
    await expect(screen().getByRole("dialog")).toBeVisible();
    await expect(dialog.getByLabelText("Name")).toHaveValue("openai-prod-eu");
    // and the keyboard is back where it left off, not on the body
    await waitFor(() => expect(cancel).toHaveFocus());
  },
};

/** And "discard" closes it. Both answers, because only asserting one is half a test. */
export const DiscardGuardThrowsItAway: Story = {
  render: () => <Editor />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.type(dialog.getByLabelText("Name"), "-eu");
    await userEvent.click(dialog.getByRole("button", { name: "Cancel" }));
    await answerDiscardPrompt(true);
    await expectSheetClosed();
  },
};

/** The header's own close button runs the same guard as Cancel. */
export const HeaderCloseRunsTheGuard: Story = {
  render: () => <Editor />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.type(dialog.getByLabelText("Name"), "-eu");
    await userEvent.click(dialog.getByRole("button", { name: /close/i }));
    await answerDiscardPrompt(true);
    await expectSheetClosed();
  },
};

/**
 * Escape is a dismissal like any other, so it prompts rather than dropping the
 * draft — and a second Escape answers the *prompt*, which is the topmost modal,
 * leaving the editor exactly where it was.
 */
export const EscapePromptsAndEscapeAgainKeepsEditing: Story = {
  render: () => <Editor />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.type(dialog.getByLabelText("Name"), "-eu");
    await userEvent.keyboard("{Escape}");
    await discardPrompt();

    await userEvent.keyboard("{Escape}");
    await waitFor(() =>
      expect(
        screen().queryByRole("dialog", { name: /discard unsaved changes/i }),
      ).not.toBeInTheDocument(),
    );
    await expect(dialog.getByLabelText("Name")).toHaveValue("openai-prod-eu");
  },
};

/** The scrim dismisses too, and it goes through the same prompt. */
export const ScrimClickRunsTheGuard: Story = {
  render: () => <Editor />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.type(dialog.getByLabelText("Name"), "-eu");
    await userEvent.click(screen().getByTestId("sheet-scrim"));
    await answerDiscardPrompt(true);
    await expectSheetClosed();
  },
};

/**
 * Two dismissals in a row raise one prompt, not a queue of them: the old
 * `window.confirm` serialised for free, a rendered dialog has to say so.
 */
export const RepeatedDismissalsRaiseOnePrompt: Story = {
  render: () => <Editor />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.type(dialog.getByLabelText("Name"), "-eu");
    await userEvent.keyboard("{Escape}");
    await discardPrompt();
    await userEvent.click(screen().getByTestId("sheet-scrim"));
    await expect(
      screen().getAllByRole("dialog", { name: /discard unsaved changes/i }),
    ).toHaveLength(1);
  },
};

/**
 * Dismissal during an in-flight save is refused, and the controls that offer it
 * say so. A sheet that vanished here would leave the operator with no way to
 * tell whether the mutation landed.
 */
export const SavingRefusesDismissal: Story = {
  render: () => <Editor saving />,
  play: async () => {
    const dialog = within(sheet());
    await userEvent.type(dialog.getByLabelText("Name"), "-eu");
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeDisabled();
    await expect(dialog.getByRole("button", { name: /close/i })).toBeDisabled();

    await userEvent.keyboard("{Escape}");
    await userEvent.click(screen().getByTestId("sheet-scrim"));
    // no prompt, and the editor is still up with the draft in it
    await expect(
      screen().queryByRole("dialog", { name: /discard unsaved changes/i }),
    ).not.toBeInTheDocument();
    await expect(dialog.getByLabelText("Name")).toHaveValue("openai-prod-eu");
  },
};

/* ---------------- UX stream (#1730) ---------------- */

// the screen key travels through context the way the app shell supplies it; a
// sheet rendered outside a provider is silent rather than mislabelled
const SCREEN = "providers";
const TARGET = "provider-edit";

/**
 * The shell with a save the story can settle either way, because the two
 * outcomes the sheet reports are read off `saving` falling back and whether
 * `errorMessage` arrived with it — the only two props a caller uses to say
 * what happened.
 */
function Instrumented({ fails = false }: { fails?: boolean }) {
  const [open, setOpen] = React.useState(true);
  const [name, setName] = React.useState("openai-prod");
  const [saving, setSaving] = React.useState(false);
  const [errorMessage, setErrorMessage] = React.useState<string | undefined>(undefined);

  // settles the round trip on the commit after it started — no timer, so the
  // story asserts a real state change rather than a scheduled one
  React.useEffect(() => {
    if (!saving) return;
    setSaving(false);
    if (fails) setErrorMessage("a provider with slug 'openai-prod' already exists in this org");
    else setOpen(false);
  }, [saving, fails]);

  return (
    <UxScreenProvider screen={SCREEN}>
      <EditorSheet
        name={TARGET}
        open={open}
        onOpenChange={setOpen}
        title="Edit provider"
        subtitle="openai-prod · openai"
        dirty={false}
        errorMessage={errorMessage}
        saveLabel="Save provider"
        canSave
        saving={saving}
        onSave={() => {
          setErrorMessage(undefined);
          setSaving(true);
        }}
      >
        <Field label="Name">
          <Input value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
      </EditorSheet>
    </UxScreenProvider>
  );
}

/**
 * Pressing save is a `form_submit`, and the round trip landing is a
 * `save_confirmed` carrying how long it took. Thirteen screens render this
 * shell, so this is the only place the dogfood week learns any of them was
 * used at all.
 */
export const SaveEmitsSubmitAndConfirmation: Story = {
  render: () => <Instrumented />,
  play: async () => {
    await userEvent.click(within(sheet()).getByRole("button", { name: "Save provider" }));
    const submit = await expectUxEvent("form_submit", TARGET);
    await expect(submit.screen).toBe(SCREEN);
    await expect(submit.outcome).toBe("ok");
    const confirmed = await expectUxEvent("save_confirmed", TARGET);
    await expect(confirmed.outcome).toBe("ok");
  },
};

/** A refused save is a second `form_submit`, and it is not a confirmation. */
export const AFailedSaveEmitsAnError: Story = {
  render: () => <Instrumented fails />,
  play: async () => {
    await userEvent.click(within(sheet()).getByRole("button", { name: "Save provider" }));
    await waitFor(() => {
      const outcomes = uxEvents()
        .filter((e) => e.action === "form_submit" && e.target === TARGET)
        .map((e) => e.outcome);
      expect(outcomes).toEqual(["ok", "error"]);
    });
    expectNoUxEvent("save_confirmed", TARGET);
  },
};

/**
 * A sheet closed without submitting is a `form_abandon` with the dwell time on
 * it — the signal the issue was filed for, and the one that separates "opened
 * by mistake" from "filled it in and gave up". It must not look like a submit.
 */
export const ClosingWithoutSavingEmitsAnAbandon: Story = {
  render: () => <Instrumented />,
  play: async () => {
    await userEvent.click(within(sheet()).getByRole("button", { name: "Cancel" }));
    await expectSheetClosed();
    const event = await expectUxEvent("form_abandon", TARGET);
    await expect(event.screen).toBe(SCREEN);
    await expect(event.outcome).toBe("cancelled");
    await expect(typeof event.duration_ms).toBe("number");
    expectNoUxEvent("form_submit", TARGET);
  },
};

/**
 * The shape five screens use (#1739): the parent holds the draft and mounts
 * the sheet only while it exists, so `open` is a bare literal and the sheet is
 * *unmounted* instead of closed. There is no closing edge to read here, and
 * reading only that edge is why those screens reported zero abandonments.
 */
function Unmountable({ mounted: initial = true }: { mounted?: boolean }) {
  const [draft, setDraft] = React.useState<string | null>(initial ? "openai-prod" : null);
  const [submitted, setSubmitted] = React.useState(false);
  return (
    <UxScreenProvider screen={SCREEN}>
      <button type="button" onClick={() => setDraft("openai-prod")}>
        open the editor
      </button>
      <button type="button" onClick={() => setDraft(null)}>
        drop the editor
      </button>
      {submitted && <p>submitted</p>}
      {draft !== null && (
        <EditorSheet
          name={TARGET}
          open
          onOpenChange={(next) => !next && setDraft(null)}
          title="Edit provider"
          subtitle="openai-prod · openai"
          dirty={false}
          saveLabel="Save provider"
          canSave
          saving={false}
          onSave={() => setSubmitted(true)}
        >
          <Field label="Name">
            <Input value={draft} onChange={(e) => setDraft(e.target.value)} />
          </Field>
        </EditorSheet>
      )}
    </UxScreenProvider>
  );
}

/**
 * An editor that goes away by being unmounted is an abandon exactly once — the
 * regression #1739 was filed for. Counted rather than merely found: the fix
 * defers the emit so a remount can cancel it, and a deferral that fired twice
 * would double every abandonment on those five screens.
 */
export const UnmountingWithoutSavingEmitsOneAbandon: Story = {
  render: () => <Unmountable />,
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "drop the editor" }));
    await expectSheetClosed();
    const event = await expectUxEvent("form_abandon", TARGET);
    await expect(event.screen).toBe(SCREEN);
    await expect(typeof event.duration_ms).toBe("number");
    expectNoUxEvent("form_submit", TARGET);
    // the queue has settled by the time the event above was waited for, so a
    // second copy would already be in it
    await expect(
      uxEvents().filter((e) => e.action === "form_abandon" && e.target === TARGET),
    ).toHaveLength(1);
  },
};

/**
 * `React.StrictMode` wraps the whole app in `src/main.tsx`, and its simulated
 * unmount right after mount looks exactly like the teardown above. Emitting
 * straight from the effect cleanup would therefore put a spurious abandon on
 * every editor opened in `bun run dev` — an abandonment for a form still on
 * screen, which is worse than the zero it replaced.
 *
 * The sheet is mounted *into* an already-mounted StrictMode rather than with
 * it, because that is the only arrangement that double-invokes: React walks
 * the newly placed subtree and only doubles what sits below a `StrictMode`
 * element it passed on the way down, so a StrictMode placed in the same commit
 * as its children doubles nothing and this story would assert against a
 * lifecycle that never happened. It is also the app's own shape — the root is
 * strict long before anyone opens an editor.
 */
export const StrictModeDoesNotInventAnAbandon: Story = {
  render: () => (
    <React.StrictMode>
      <Unmountable mounted={false} />
    </React.StrictMode>
  ),
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "open the editor" }));
    // the submit is the proof that the double-invoke has been and gone: it
    // cannot be pressed before the sheet has mounted, remounted and settled
    await userEvent.click(within(sheet()).getByRole("button", { name: "Save provider" }));
    await expectUxEvent("form_submit", TARGET);
    await waitFor(() => expect(screen().getByText("submitted")).toBeVisible());
    expectNoUxEvent("form_abandon", TARGET);
  },
};

/**
 * And the real abandonment underneath it still arrives, exactly once. The
 * deferral is what separates the two — a cancelled deferral must not swallow
 * the teardown that follows it, and an uncancelled one must not double it.
 */
export const StrictModeStillReportsARealAbandonOnce: Story = {
  render: () => (
    <React.StrictMode>
      <Unmountable mounted={false} />
    </React.StrictMode>
  ),
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "open the editor" }));
    await waitFor(() => expect(sheet()).toBeVisible());
    await userEvent.click(screen().getByRole("button", { name: "drop the editor" }));
    await expectSheetClosed();
    await expectUxEvent("form_abandon", TARGET);
    await expect(
      uxEvents().filter((e) => e.action === "form_abandon" && e.target === TARGET),
    ).toHaveLength(1);
  },
};

/**
 * A form nobody opened has nothing to report. The deferral has to keep that
 * true: an unmount is an abandonment only when there was a dwell to abandon,
 * so a sheet mounted closed and then thrown away — a screen unmounting under
 * it, a route change — must stay silent rather than invent one.
 */
function MountedClosed() {
  const [mounted, setMounted] = React.useState(true);
  return (
    <UxScreenProvider screen={SCREEN}>
      <button type="button" onClick={() => setMounted(false)}>
        drop the editor
      </button>
      {mounted && (
        <EditorSheet
          name={TARGET}
          open={false}
          onOpenChange={() => {}}
          title="Edit provider"
          subtitle="openai-prod · openai"
          dirty={false}
          saveLabel="Save provider"
          canSave
          saving={false}
          onSave={() => {}}
        >
          <Field label="Name">
            <Input value="openai-prod" onChange={() => {}} />
          </Field>
        </EditorSheet>
      )}
      <p>dropped: {String(!mounted)}</p>
    </UxScreenProvider>
  );
}

export const UnmountingAnUnopenedEditorEmitsNothing: Story = {
  render: () => <MountedClosed />,
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "drop the editor" }));
    await waitFor(() => expect(screen().getByText("dropped: true")).toBeVisible());
    expectNoUxEvent("form_abandon", TARGET);
  },
};
