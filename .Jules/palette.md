## 2024-07-24 - Icon-only buttons and loading states
**Learning:** Found a recurring pattern where icon-only action buttons (like Delete) use `title` but lack `aria-label` and `focus-visible` states, making them difficult to navigate via keyboard and less accessible to screen readers. Also, mutation buttons like 'Create' or 'Delete' often just use `disabled` states without visual feedback (loading spinners).
**Action:** Always add `aria-label` with context (e.g. "Delete key [name]") and `focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring` to raw `<button>` elements. Use inline `<Loader2 className="mr-2 h-4 w-4 animate-spin" />` in submit buttons to provide immediate visual feedback.
## 2026-07-25 - Missing focus rings on generic inputs
**Learning:** Found that custom/generic input implementations like `SearchInput` in the app can be missing the standard Shadcn/Tailwind focus rings (`focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-none`) that are present on standard `Input` components, leading to broken keyboard navigation for primary page actions.
**Action:** Always verify keyboard accessibility (`focus-visible`) and semantic HTML tags (`type="search"`) on custom input wrappers and icon-only buttons across the application.

I am learning that Linear tickets are no longer used for PR titles.

## 2024-05-18 - [Missing a11y & async feedback on Row Actions]
**Learning:** Found a recurring pattern in data tables where icon-only action buttons (like Delete) in rows lack `aria-label` attributes and keyboard focus states (`focus-visible` classes). Additionally, confirmation dialogs for these destructive row actions often lack visual loading indicators (`Loader2` from lucide-react) while the asynchronous mutation is pending. This makes keyboard navigation difficult and leaves users wondering if their delete request was registered.
**Action:** When working on data table rows or generic item listings, explicitly verify that all icon-only buttons include an `aria-label` and `focus-visible` classes. Also, always ensure the corresponding confirmation dialogs provide visual loading feedback via `Loader2` during the mutation.

## 2026-07-29 - [Global refresh visual indicator]
**Learning:** Relying purely on a manual loading state in global headers can mismatch UI states if a user triggers an implicit background fetch while also hitting the 'Refresh' button. We need to tie top-level refresh visual indicators to the global `useIsFetching` state.
**Action:** Next time, always check if manual refresh indicators can be augmented with the data library's global fetching hooks (like TanStack's `useIsFetching()`) to represent all ongoing network states.
## 2024-07-31 - Keyboard navigation in RowIconButton & SortLabel
**Learning:** Shared UI components used across lists and grids (like `RowIconButton` and `SortLabel` in `screen.tsx`) didn't have keyboard focus indicators, making the entire app's tables inaccessible to keyboard-only users. Because these components are heavily reused, adding `focus-visible` to these primitives fixes accessibility across many pages (e.g., Users, Keys) at once.
**Action:** When auditing list/table accessibility, verify focus styles on shared generic row action buttons. Ensure the `focus-visible:ring-1` pattern is applied to custom primitives that use `button` HTML tags.
## 2024-06-03 - Missing keyboard focus states on raw `<button>` icon actions

**Learning:** There's a recurring pattern in the codebase of using raw `<button>` elements (instead of the `<Button>` component) for small destructive/inline actions (like trash or remove icons). While they often have hover states, they frequently miss keyboard focus states (`focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring`) and semantic `aria-label`s, breaking keyboard navigation and screen reader accessibility for these critical actions.
**Action:** When reviewing or creating new inline icon buttons, explicitly check for and enforce both `aria-label` and `focus-visible` Tailwind classes if the `<Button>` component (which provides these by default) isn't being used.

## 2026-08-03 - [Missing accessibility on generic tab filters and action icons]
**Learning:** Found a recurring pattern in the app (such as the origins filter and delete model icons in `Models.tsx`) where custom list filters and table row action icons use raw `<button>` elements instead of the standard `<Button>` component. As a result, they frequently lack critical accessibility attributes like `aria-label` (making icon-only buttons unreadable by screen readers) and `focus-visible` states (making keyboard navigation invisible).
**Action:** Always check custom filter tabs and raw `<button>` elements used for icons in list views. Ensure they explicitly include `aria-label` and `focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring` classes for accessible keyboard navigation and screen reader support.
## 2026-08-06 - Missing focus-visible states on custom components

**Learning:** When building custom icon-only buttons or interactive elements (like the close buttons in Dialogs and Sheets, or the Add/Delete buttons in ScopeSwitcher), developers often forget to add explicit `focus-visible:` Tailwind states for keyboard navigation. While `hover:` states are usually present, keyboard accessibility relies heavily on `focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring` being applied to these custom `<button>` elements to ensure they are visually distinct when tabbed to.

**Learning (follow-up):** Sweeping the leaf components first missed the component that mattered most. `nav-sidebar.tsx` builds every nav item from a shared `itemBase` class string that had no focus state, so the dashboard's primary navigation — the first thing a keyboard user tabs into — was the least accessible part of the app while the dialogs were already fixed. A shared class string is invisible to a per-`<button>` search.

**Action:** Always check custom components containing raw `<button>` elements for `focus-visible` utility classes. Search the shared class *constants* too (`itemBase`-style strings, `cva` bases), not just the JSX, and start with navigation and filters rather than leaf icons — they carry more keyboard traffic.

## 2026-08-08 - Missing focus states on raw buttons in list components
**Learning:** Some custom UI lists/tables (like the version history panels in `SkillsRepository.tsx` and `PromptRepository.tsx`) rely on raw `<button>` elements to act as interactive list rows. While these buttons often have `hover:` styling, they were missing the essential `focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring` Tailwind utility classes necessary to indicate keyboard focus, breaking keyboard navigation.
**Action:** When auditing or implementing interactive lists that use raw `<button>` elements rather than standard `<Button>` components, explicitly verify they include the standard `focus-visible:ring-1` focus ring pattern to ensure keyboard accessibility.
