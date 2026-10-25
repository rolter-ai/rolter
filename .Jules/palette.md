## 2024-08-09 - Added Loading States to Connector Actions

**Learning:** For list items mapping over a shared `useMutation`, using `isPending` without checking `variables` disables the action across all list items globally, which is confusing UX.
**Action:** When working with row-level actions in a mapped list, always qualify loading/disabled states using `mutation.variables === row.id` to localize the pending visual state to just the active row. Ensure buttons provide visual feedback (e.g., `<Loader2 className="animate-spin" />`) during async operations and add `disabled:opacity-50 disabled:pointer-events-none` to prevent duplicate submissions.

## 2026-08-10 - [Keyboard Accessibility: Focus Rings on Buttons]

**Learning:** Many custom-styled icon buttons and list-item buttons lacked visible focus states for keyboard navigation, making the interface harder to use for accessibility. We applied consistent `focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring` Tailwind classes across multiple pages.
**Action:** Ensure all future custom interactive elements explicitly handle keyboard focus states using existing Tailwind ring utilities.

## 2023-10-27 - Disabled styling on buttons

**Learning:** When adding `disabled={true}` to buttons, they might not inherently visually reflect it correctly without explicit Tailwind styling. `disabled:pointer-events-none disabled:opacity-50` ensures that a disabled button is visually dimmed and doesn't trigger hover events.
**Action:** When updating standard HTML buttons or custom components to respond to `isPending` states, ensure `disabled:pointer-events-none disabled:opacity-50` are added to the Tailwind classes.

## 2023-10-27 - Loading States for Async Mutations

**Learning:** When adding visual loading states (`<Loader2 className="mr-2 h-4 w-4 animate-spin" />`) or disabling buttons for row-level actions in a mapped list sharing a `useMutation`, you must qualify the loading/disabled state using `mutation.variables === row.id` (or similar, depending on the mutation payload structure) to avoid disabling all items globally when any one row is mutating. Also, `import { Loader2 } from "lucide-react";` needs to be carefully merged with existing lucide-react imports if present to pass the TypeScript linter.
**Action:** Always check the payload structure of the `mutate` call and verify `mutation.isPending && mutation.variables === id` before showing row-level loading states.

## 2024-11-20 - Global vs Item-level loading and delete dialogs

**Learning:** Destructive inline actions (like 'Delete' in lists) without dedicated modals should be guarded by `window.confirm(...)` before mutating. When applying pending visual states to row-level actions in a mapped list sharing a `useMutation`, qualify the loading/disabled state using `mutation.variables === row.id` to avoid disabling all items globally.
**Action:** When updating mapped list actions, ensure `window.confirm` is used for unmodalized deletes and that `isPending` checks verify the item being mutated using `mutation.variables`.

## 2024-08-11 - Focus ring on custom input wrappers

**Learning:** For custom input wrappers, where the actual `<input>` uses `outline-none`, keyboard focus rings will be completely invisible. Instead of adding a ring directly to the `input` (which might misalign with the styled wrapper box), the outer styled `div` or `label` container must have `focus-within:border-ring focus-within:ring-1 focus-within:ring-ring` applied. This is critical for keyboard accessibility in complex React/Tailwind components like those in `FilterSearchList`.
**Action:** When creating or fixing custom input wrappers that remove default outlines, always ensure the focus indicator is applied to the visible boundary container using Tailwind's `focus-within` utilities.

## 2024-05-18 - [ModelSheet Buttons]

**Learning:** Icon-only remove buttons for dynamically added rows (like custom params/headers) frequently miss keyboard focus indicators (`focus-visible`) and can have inconsistent hover styles if copied from older patterns.
**Action:** Always ensure `focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring` is applied to interactive list-row action buttons to support keyboard navigation.

## 2026-09-09 - [Added missing loading states in Playground]

**Learning:** Users lack visibility during async playground actions if only the disabled state is used without a loading spinner.
**Action:** When creating async buttons in complex forms like the playground, always pair the `disabled={busy}` property with a `<Loader2 className="animate-spin" />` state to provide clear visual feedback.

## 2026-09-10 - SCIM Token Revocation ConfirmDialog

**Learning:** Hand-rolled `<Dialog>` implementations for destructive actions often miss inline error displays, UX telemetry (`useFormTelemetry`), and pending spinner states on the confirm button.
**Action:** Always replace hand-rolled destructive dialogs with the shared `ConfirmDialog` component (`name="<action-slug>"`), passing `pending` and `error` from the mutation so failures are reported directly inside the open dialog.

## 2026-09-11 - Users Status Segmented Control Primitive

**Learning:** Replacing hand-rolled tab buttons with the `<Segmented />` primitive standardizes accessibility across screens (`role="radiogroup"` and `role="radio"`).
**Action:** When replacing custom tab/button groups with `<Segmented />`, update story assertions to query `getByRole("radio", { name: ... })` instead of `getByRole("button", ...)` and include an explicit `ariaLabel` for proper screen reader context.

## 2026-10-10 - Models Origin Segmented Filter Primitive

**Learning:** Hand-rolled filter buttons on pages like `Models.tsx` lacked standard radio group keyboard navigation (`roving tabindex`) and screen reader announcements (`role="radiogroup"` / `role="radio"`).
**Action:** Replace custom filter buttons with the `<Segmented />` primitive from `ui/src/components/ui/segmented.tsx`, ensuring proper `ariaLabel` localization and updating story assertions to use `getByRole("radiogroup")` and `getByRole("radio")`.

## 2026-10-11 - AccessProfiles DeleteIconButton Primitive

**Learning:** Hand-rolled icon buttons using GatedButton with Trash2 and Loader2 manually recreate button styling and pending state checks, missing standard design system traits like hover border states (hover:border-[color:var(--status-danger)]).
**Action:** Replace custom gated icon delete buttons on list/card items with DeleteIconButton from @/components/ui/delete-icon-button.

## 2026-10-12 - Inline Loading Spinners on Guardrail Dialog Submit Buttons

**Learning:** Form dialog submit buttons in `GuardrailProviders.tsx` and `GuardrailRules.tsx` changed label text to `publishing` during async save mutations but lacked inline `<Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />` icons.
**Action:** Always pair `disabled={pending}` submit buttons in form dialogs with an inline `<Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />` icon alongside translated pending text for explicit visual feedback.

## 2026-10-13 - Rbac Custom Roles DeleteIconButton Primitive

**Learning:** Hand-rolled delete buttons using `RowIconButton` with an ignored `danger` prop lack standard design system hover traits (`hover:border-[color:var(--status-danger)]` and `hover:text-[color:var(--status-danger-text)]`).
**Action:** Always replace hand-rolled delete buttons on list and card items with `DeleteIconButton` from `@/components/ui/delete-icon-button`.

## 2026-10-20 - GroupMappings DeleteIconButton Primitive

**Learning:** Hand-rolled row icon buttons using `RowIconButton` with an inline `Trash2` icon manually recreate delete button styling and miss standardized hover states (`hover:border-[color:var(--status-danger)]` / `hover:text-[color:var(--status-danger-text)]`) and pending spinner feedback.
**Action:** Replace hand-rolled row delete buttons with `DeleteIconButton` from `@/components/ui/delete-icon-button`, ensuring `pending={remove.isPending && remove.variables === row.id}` is set to localize spinner visual state.

## 2026-10-16 - Repository Screens DeleteIconButton Primitive

**Learning:** `PromptRepository.tsx` and `SkillsRepository.tsx` hand-rolled icon delete buttons with `GatedButton` or `Button` and raw `Trash2`, missing standard design system traits like hover border states (`hover:border-[color:var(--status-danger)]`).
**Action:** Always use the shared `DeleteIconButton` primitive for icon delete buttons across repository headers and editor row items.


## 2026-10-21 - Playground DeleteIconButton Primitive

**Learning:** Hand-rolled icon delete controls (`<Button size="icon" variant="ghost">` or `<button>`) for column, attachment, and list-row removals miss standard design system hover border states (`hover:border-[color:var(--status-danger)]` / `hover:text-[color:var(--status-danger-text)]`) and focus indicators.
**Action:** Replace custom icon delete buttons in complex form and workspace surfaces with `DeleteIconButton` from `@/components/ui/delete-icon-button`.
