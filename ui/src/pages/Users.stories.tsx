import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import Users from "./Users";
import {
  Harness,
  LOADING_LABEL,
  Toasted,
  answerSecretClosePrompt,
  cancelConfirmation,
  clickWhenEnabled,
  confirmation,
  confirmDestructive,
  expectClosesWithoutPrompting,
  expectEmptyState,
  expectAllowed,
  expectGateAnswered,
  expectLoadError,
  expectNoUxEvent,
  expectRefused,
  expectSheetClosed,
  expectListStateInViewport,
  expectListTable,
  expectNoFalseEmpty,
  expectSkeleton,
  expectToast,
  expectUxEvent,
  json,
  NEEDS_SUPERADMIN,
  openOptions,
  pending,
  pickOption,
  recordUxEvents,
  recording,
  routes,
  scoped,
  sheet,
  answerDiscardPrompt,
  uxEvents,
  type FetchStub,
  type Recorder,
} from "./story-harness";
import type {
  AccessProfileRow,
  Invitation,
  MembershipRow,
  UpdateUserInput,
  UserRow,
} from "@/lib/api";
import { AuthProvider } from "@/lib/auth";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { expectFieldHint } from "@/lib/story-hints";
import {
  atMobile,
  atSplit,
  expectInViewport,
  expectNoHorizontalOverflow,
  expectNotTruncated,
} from "@/lib/story-viewport";
import { UxScreenProvider } from "@/lib/ux-react";

const USERS: UserRow[] = [
  {
    id: "user-1",
    email: "ada@example.com",
    is_superadmin: true,
    deactivated_at: null,
    created_at: "2026-01-04T00:00:00Z",
  },
  {
    id: "user-2",
    email: "grace@example.com",
    is_superadmin: false,
    deactivated_at: null,
    created_at: "2026-03-11T00:00:00Z",
  },
  {
    id: "user-3",
    email: "former@example.com",
    is_superadmin: false,
    deactivated_at: "2026-06-01T00:00:00Z",
    created_at: "2026-02-02T00:00:00Z",
  },
];

const MEMBERSHIPS: MembershipRow[] = [
  {
    id: "m-1",
    user_id: "user-1",
    org_id: "org-1",
    team_id: null,
    project_id: null,
    role: "admin",
    created_at: "2026-01-04T00:00:00Z",
  },
  {
    id: "m-2",
    user_id: "user-2",
    org_id: null,
    team_id: "team-1",
    project_id: null,
    role: "member",
    created_at: "2026-03-11T00:00:00Z",
  },
  {
    id: "m-3",
    user_id: "user-3",
    org_id: null,
    team_id: null,
    project_id: "project-1",
    role: "viewer",
    created_at: "2026-02-02T00:00:00Z",
  },
];

// the per-grant controls name the grant they act on (#1214, #2053)
// a second org admin: the grant that makes ada's one not the last
const GRACE_ORG_ADMIN: MembershipRow = {
  id: "m-9",
  user_id: "user-2",
  org_id: "org-1",
  team_id: null,
  project_id: null,
  role: "admin",
  created_at: "2026-04-01T00:00:00Z",
};
const REVOKE_ADA = "Revoke Admin on the whole organization from ada@example.com";
const CHANGE_ADA = "Change ada@example.com's Admin role on the whole organization";
// what the control plane answers when a revoke would leave the org without one (#2311)
const lastAdminRefusal = () =>
  json(
    {
      error: {
        code: "last_org_admin",
        message: "this is the organization's last admin grant",
      },
    },
    409,
  );
const REVOKE_GRACE = "Revoke Member on the Platform team from grace@example.com";
const CHANGE_GRACE = "Change grace@example.com's Member role on the Platform team";
const revokeInvitationName = (email: string) => `Revoke the invitation for ${email}`;

const DAY = 86_400_000;
const inDays = (days: number) => new Date(Date.now() + days * DAY).toISOString();

// what `GET /orgs/{org}/invitations` answers: every invitation of the org,
// newest first, the spent ones (accepted, revoked) among the pending. the
// screen lists only the four that have neither
const INVITATIONS: Invitation[] = [
  {
    id: "inv-1",
    org_id: "org-1",
    email: "newcomer@example.com",
    role: "member",
    team_id: null,
    project_id: "project-1",
    invited_by: "user-1",
    invited_by_email: "ada@example.com",
    expires_at: inDays(5),
    accepted_at: null,
    revoked_at: null,
    created_at: "2026-09-25T00:00:00Z",
    mail_status: "not_configured",
    mail_attempted_at: null,
    mail_error: null,
  },
  {
    id: "inv-2",
    org_id: "org-1",
    email: "lead@example.com",
    role: "admin",
    team_id: "team-1",
    project_id: null,
    invited_by: "user-2",
    invited_by_email: "grace@example.com",
    expires_at: inDays(2),
    accepted_at: null,
    revoked_at: null,
    created_at: "2026-09-24T00:00:00Z",
    mail_status: "not_configured",
    mail_attempted_at: null,
    mail_error: null,
  },
  {
    id: "inv-3",
    org_id: "org-1",
    email: "everyone@example.com",
    role: "viewer",
    team_id: null,
    project_id: null,
    invited_by: null,
    invited_by_email: null,
    expires_at: inDays(6),
    accepted_at: null,
    revoked_at: null,
    created_at: "2026-09-23T00:00:00Z",
    mail_status: "not_configured",
    mail_attempted_at: null,
    mail_error: null,
  },
  {
    id: "inv-4",
    org_id: "org-1",
    email: "stale@example.com",
    role: "member",
    team_id: null,
    project_id: null,
    invited_by: "user-9",
    invited_by_email: "dana@elsewhere.example",
    expires_at: "2026-01-10T00:00:00Z",
    accepted_at: null,
    revoked_at: null,
    created_at: "2026-01-03T00:00:00Z",
    mail_status: "not_configured",
    mail_attempted_at: null,
    mail_error: null,
  },
  {
    id: "inv-5",
    org_id: "org-1",
    email: "joined@example.com",
    role: "member",
    team_id: "team-1",
    project_id: null,
    invited_by: "user-1",
    invited_by_email: "ada@example.com",
    expires_at: inDays(3),
    accepted_at: "2026-09-26T00:00:00Z",
    revoked_at: null,
    created_at: "2026-09-22T00:00:00Z",
    mail_status: "not_configured",
    mail_attempted_at: null,
    mail_error: null,
  },
  {
    id: "inv-6",
    org_id: "org-1",
    email: "withdrawn@example.com",
    role: "viewer",
    team_id: null,
    project_id: "project-1",
    invited_by: "user-1",
    invited_by_email: "ada@example.com",
    expires_at: inDays(4),
    accepted_at: null,
    revoked_at: "2026-09-26T00:00:00Z",
    created_at: "2026-09-21T00:00:00Z",
    mail_status: "not_configured",
    mail_attempted_at: null,
    mail_error: null,
  },
];

const loaded = routes([
  ["/memberships", () => MEMBERSHIPS],
  ["/users", () => USERS],
]);
const empty = routes([
  ["/memberships", () => []],
  ["/users", () => []],
]);

const meta = {
  title: "Screens/Users",
  component: Users,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Users>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // an active and a deactivated account render side by side; the status tabs
    // are the only place the counts are visible
    await expect(await canvas.findByText("ada@example.com")).toBeInTheDocument();
    await expect(canvas.getByText("former@example.com")).toBeInTheDocument();
    await expectListTable(canvasElement, "Users");

    // each grant reads as its role and its scope by name: the team and the
    // project, never the head of a uuid (#2053)
    const row = (email: string) =>
      within(canvas.getByText(email).closest('[role="row"]') as HTMLElement);
    await waitFor(() => expect(row("grace@example.com").getByText("Platform")).toBeVisible());
    await expect(row("grace@example.com").getByText("Member")).toBeVisible();
    await expect(row("former@example.com").getByText("Gateway")).toBeVisible();
    await expect(row("former@example.com").getByText("Viewer")).toBeVisible();
    await expect(row("ada@example.com").getByText("Whole organization")).toBeVisible();
    await expect(canvas.queryByText(/project-1|team-1/)).toBeNull();
  },
};

/**
 * A named account leads with its name and keeps the email as secondary text; an
 * account with no name keeps the email as its headline, so a row never goes
 * blank (#2434). The bio says who to ask about what, and search matches names.
 */
export const ShowsDisplayNamesBesideTheEmail: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/memberships", () => MEMBERSHIPS],
        [
          "/users",
          () =>
            USERS.map((u) =>
              u.id === "user-2"
                ? { ...u, display_name: "Grace Hopper", bio: "Ask me about the compiler routes" }
                : u,
            ),
        ],
      ])}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const name = await canvas.findByText("Grace Hopper");
    const row = within(name.closest('[role="row"]') as HTMLElement);
    // the email is still there, as the secondary line
    await expect(row.getByText("grace@example.com")).toBeVisible();
    await expect(row.getByText("Ask me about the compiler routes")).toBeVisible();
    // an unnamed account is headed by its email
    await expect(canvas.getByText("ada@example.com")).toBeVisible();
    // the search box finds a person by name
    await userEvent.type(canvas.getByRole("searchbox"), "hopper");
    await waitFor(() => expect(canvas.queryByText("ada@example.com")).toBeNull());
    await expect(canvas.getByText("Grace Hopper")).toBeVisible();
  },
};

/**
 * The grants are a read of their own, so while it is still coming a row is not
 * a person with "no role in this org" (#2211).
 */
export const RolesStillLoading: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships") ? new Promise<Response>(() => {}) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("grace@example.com")).toBeInTheDocument();
    await expect(canvas.queryByText("no role in this org")).toBeNull();
  },
};

/** A failed grants read says so on every row, under the screen's own `LoadError`. */
export const RolesFailedToLoad: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships")
          ? json({ error: { message: "store unavailable" } }, 500)
          : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /store unavailable/);
    await waitFor(() => expect(canvas.getAllByText("roles not loaded")).toHaveLength(3));
    await expect(canvas.queryByText("no role in this org")).toBeNull();
  },
};

export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
    await expectNoFalseEmpty(canvasElement, /No users yet/);
  },
};

export const Empty: Story = {
  render: () => (
    <Harness fetchStub={empty}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No users yet/, /Invite user/);
  },
};

/**
 * A first-run admin on a phone (#2362). The tables scroll sideways inside their
 * cards below their column floor, and the empty state is the screen's one call
 * to action: it has to be in the part of the table a reader can see, not
 * centred in a 960px row that begins off the right edge. Both tables, since the
 * pending invitations sit under the users and are their own scroller.
 */
export const EmptyIsOnScreenAtPhoneWidth: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={empty}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectListStateInViewport(canvasElement, en.screens["gov-users"].title, {
      says: /No users yet/,
      cta: /invite user/i,
    });
    await expectListStateInViewport(canvasElement, en.pages.users.invitations.title, {
      says: /No pending invitations/,
      cta: /invite user/i,
    });
    await expectNoHorizontalOverflow();
  },
};

/** The Russian copy is the longer one; its title, description and button still fit the card. */
export const EmptyIsOnScreenAtPhoneWidthInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={empty}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectListStateInViewport(canvasElement, ru.screens["gov-users"].title, {
      says: new RegExp(ru.pages.users.emptyTitle),
      cta: new RegExp(ru.pages.users.emptyAction),
    });
    await expectNoHorizontalOverflow();
  },
};

/** The loading skeleton is the same row as the empty state, so it is held to the same edge. */
export const LoadingIsOnScreenAtPhoneWidth: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={pending}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectListStateInViewport(canvasElement, en.screens["gov-users"].title);
    await expectListStateInViewport(canvasElement, en.pages.users.invitations.title);
    await expectNoHorizontalOverflow();
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(/do not have access to users/i)).toBeInTheDocument();
    await expectNoFalseEmpty(canvasElement, /No users yet/);
  },
};

/**
 * A server error is not an empty organization. `LoadError` names the failure
 * with the control plane's own message, and "Try again" asks again for both
 * reads the screen holds, the users and their grants, then the table fills in.
 */
let unavailable: Recorder;
let storeHealed = false;
export const LoadFailureOffersARetry: Story = {
  render: () => {
    storeHealed = false;
    unavailable = recording(
      scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) return json([]);
        if (url.includes("/memberships")) return json(MEMBERSHIPS);
        if (url.includes("/users")) {
          return storeHealed ? json(USERS) : json({ error: { message: "store unavailable" } }, 500);
        }
        return json([]);
      }),
    );
    return (
      <Harness fetchStub={unavailable.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectLoadError(canvasElement, /store unavailable/);
    await expectNoFalseEmpty(canvasElement, /No users yet/);
    // the failed read is not a count either
    await expect(canvas.queryByRole("radio", { name: /^All \d/ })).toBeNull();

    const reads = (path: string) =>
      unavailable.calls.filter((c) => c.method === "GET" && c.url.endsWith(path)).length;
    const [usersBefore, grantsBefore] = [reads("/users"), reads("/memberships")];
    storeHealed = true;
    await userEvent.click(
      within(canvas.getByRole("alert")).getByRole("button", { name: "Try again" }),
    );

    await waitFor(() => expect(reads("/users")).toBe(usersBefore + 1));
    await waitFor(() => expect(reads("/memberships")).toBe(grantsBefore + 1));
    await expect(await canvas.findByText("ada@example.com")).toBeVisible();
    await waitFor(() => expect(canvas.queryByRole("alert")).toBeNull());
    await expect(canvas.getByRole("radio", { name: "All 3" })).toBeInTheDocument();
  },
};

export const FiltersToDeactivatedAccounts: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("ada@example.com")).toBeInTheDocument();
    await userEvent.click(canvas.getByRole("radio", { name: /deactivated/i }));
    await expect(canvas.getByText("former@example.com")).toBeInTheDocument();
    await expect(canvas.queryByText("ada@example.com")).not.toBeInTheDocument();
  },
};

/** a filter option's label without the count the screen appends to it */
const filterWord = (radio: HTMLElement) => (radio.textContent ?? "").replace(/\s*\d+$/, "");

/**
 * One word for one state. The status filter said "deactivated" and the row
 * pill said "Blocked" for the same accounts, and the filter was lowercase
 * where the pill was capitalised by a CSS class (#2059). Both now read the
 * word the confirmation uses, capitalised in the catalog rather than by CSS.
 */
export const OneWordForTheDeactivatedState: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const table = await usersTable(canvasElement);
    await canvas.findByText("former@example.com");

    const deactivated = canvas.getByRole("radio", { name: "Deactivated 1" });
    const pill = rowOf(table, "former@example.com").getByText("Deactivated");
    await expect(filterWord(deactivated)).toBe(pill.textContent);
    const active = canvas.getByRole("radio", { name: "Active 2" });
    await expect(within(table).getAllByText("Active")).toHaveLength(2);
    await expect(filterWord(active)).toBe("Active");

    // the catalog holds the capital, so no label leans on `text-transform`
    for (const el of [...canvas.getAllByRole("radio"), pill]) {
      await expect(el.textContent).toMatch(/^\p{Lu}/u);
      await expect(getComputedStyle(el).textTransform).toBe("none");
    }
    // the word the confirmation uses is the word on screen, and the old one is gone
    await userEvent.click(canvas.getByRole("button", { name: "Deactivate grace@example.com" }));
    await expect(
      await within(document.body).findByRole("heading", { name: "Deactivate grace@example.com?" }),
    ).toBeInTheDocument();
    await cancelConfirmation();
    await expect(canvasElement.textContent).not.toMatch(/blocked/i);
  },
};

/**
 * The same in Russian, where the filter is a plural ("Заблокированные") and the
 * pill a singular ("Заблокирован"). The pill used to borrow the filter's plural
 * for an active account, and the filter said "отключённые" beside a pill that
 * said "заблокирован".
 */
export const OneWordForTheDeactivatedStateInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const copy = ru.pages.users;
    const canvas = within(canvasElement);
    // the locale decorator switches language from an effect, after first paint
    const table = await canvas.findByRole("table", { name: ru.screens["gov-users"].title });
    const deactivated = await canvas.findByRole("radio", {
      name: new RegExp(`^${copy.statusDeactivated} 1$`),
    });
    await canvas.findByText("former@example.com");

    const pill = rowOf(table, "former@example.com").getByText(copy.rowStatusDeactivated);
    await expect(filterWord(deactivated).toLowerCase()).toMatch(
      new RegExp(`^${pill.textContent?.toLowerCase()}`),
    );
    // an active account reads "Активен", not the filter's "Активные"
    await expect(within(table).getAllByText(copy.rowStatusActive)).toHaveLength(2);
    await expect(canvas.getByRole("radio", { name: `${copy.statusActive} 2` })).toBeInTheDocument();
    for (const el of [...canvas.getAllByRole("radio"), pill]) {
      await expect(el.textContent).toMatch(/^\p{Lu}/u);
    }
    await expect(canvasElement.textContent).not.toMatch(/отключ/i);
  },
};

/**
 * The chip is the person's, not the row's. It was picked by position, so a
 * search or a status tab recoloured everyone left on screen (#2059); it is now
 * derived from the id, and a person keeps the colour they had.
 */
export const AvatarColourFollowsThePerson: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("ada@example.com");
    // the chip is the one element that reads the address's first two letters
    const chip = (email: string) =>
      canvas.getByText(email.slice(0, 2).toUpperCase()).style.background;
    const [ada, grace, former] = [
      chip("ada@example.com"),
      chip("grace@example.com"),
      chip("former@example.com"),
    ];
    for (const colour of [ada, grace, former])
      await expect(colour).toMatch(/^var\(--avatar-[1-6]\)$/);
    await expect(new Set([ada, grace, former]).size).toBe(3);

    // a search leaves grace alone in the table, at the row where ada was
    const search = canvas.getByRole("searchbox", { name: "Search users" });
    await userEvent.type(search, "grace");
    await waitFor(() => expect(canvas.queryByText("ada@example.com")).toBeNull());
    await expect(chip("grace@example.com")).toBe(grace);

    // and the status tab that leaves only the deactivated account does the same
    await userEvent.clear(search);
    await userEvent.click(canvas.getByRole("radio", { name: /^Deactivated/ }));
    await waitFor(() => expect(canvas.queryByText("grace@example.com")).toBeNull());
    await expect(chip("former@example.com")).toBe(former);
  },
};

/**
 * The superadmin marker is a neutral `Badge`: no folk-red border, no 9px
 * uppercase (#2059). Red is left to state, so the loaded screen has one red
 * mark, the deactivated pill, and no row action is red.
 */
export const SuperadminIsANeutralBadge: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const table = await usersTable(canvasElement);
    const badge = await within(table).findByText("Superadmin");
    await expect(within(table).getAllByText("Superadmin")).toHaveLength(1);
    await expect(rowOf(table, "ada@example.com").getByText("Superadmin")).toBe(badge);
    await expect(badge).toHaveClass("text-muted-foreground");
    await expect(getComputedStyle(badge).textTransform).toBe("none");
    await expect(getComputedStyle(badge).fontSize).toBe("11px");
  },
};

export const TheLoadedScreenCarriesOneRedMark: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const table = await usersTable(canvasElement);
    await within(table).findByText("former@example.com");
    // the colours red can be drawn in, read the way the page resolves them
    const resolve = (token: string) => {
      const probe = document.createElement("span");
      probe.style.color = `var(${token})`;
      document.body.append(probe);
      const colour = getComputedStyle(probe).color;
      probe.remove();
      return colour;
    };
    const reds = ["--status-danger-text", "--status-danger", "--red-folk-text", "--red-folk"].map(
      resolve,
    );
    // a mark is a control or a run of text drawn in red; an icon or a dot only inherits it
    const marks = [...table.querySelectorAll<HTMLElement>("button, span")].filter((el) => {
      const own = [...el.childNodes].some((n) => n.nodeType === Node.TEXT_NODE && n.textContent);
      return (el.tagName === "BUTTON" || own) && reds.includes(getComputedStyle(el).color);
    });
    await expect(marks.map((el) => el.textContent)).toEqual(["Deactivated"]);

    // the control that deactivates reads like the one beside it
    const deactivate = within(table).getByRole("button", { name: "Deactivate grace@example.com" });
    const edit = within(table).getByRole("button", { name: "Edit grace@example.com" });
    await expect(getComputedStyle(deactivate).color).toBe(getComputedStyle(edit).color);
  },
};

/**
 * Beside the status filter the search field was squeezed to the width of its
 * icon at 375px, and its placeholder could not be read (#2059). It takes the
 * row to itself and the filter wraps underneath.
 */
async function expectSearchUsable(canvasElement: HTMLElement, name: string): Promise<void> {
  const canvas = within(canvasElement);
  const search = await canvas.findByRole<HTMLInputElement>("searchbox", { name });
  await canvas.findByText("ada@example.com");
  const field = search.parentElement as HTMLElement;
  const toolbar = field.parentElement as HTMLElement;
  await expect(Math.round(field.getBoundingClientRect().width)).toBe(
    Math.round(toolbar.getBoundingClientRect().width),
  );

  // the placeholder is readable: the room inside the field is the width of its words
  const style = getComputedStyle(search);
  const probe = document.createElement("span");
  probe.style.cssText = "position:absolute;visibility:hidden;white-space:pre";
  probe.style.font = style.font;
  probe.textContent = search.placeholder;
  document.body.append(probe);
  const words = probe.getBoundingClientRect().width;
  probe.remove();
  const room = search.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  await expect(room).toBeGreaterThanOrEqual(words);

  // the filter and the invite button stay on screen, under it
  const filter = canvas.getByRole("radiogroup");
  await expect(filter.getBoundingClientRect().top).toBeGreaterThanOrEqual(
    field.getBoundingClientRect().bottom,
  );
  await expect(filter.getBoundingClientRect().right).toBeLessThanOrEqual(window.innerWidth);
  await expectNoHorizontalOverflow();

  await userEvent.type(search, "grace");
  await expect(search).toHaveValue("grace");
  await waitFor(() => expect(canvas.queryByText("ada@example.com")).toBeNull());
}

export const SearchIsUsableAtPhoneWidth: Story = {
  ...atMobile,
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSearchUsable(canvasElement, "Search users");
  },
};

/** In Russian the filter is wider and already wrapped; the field still takes the row and nothing overflows. */
export const SearchIsUsableAtPhoneWidthInRussian: Story = {
  ...atMobile,
  globals: { ...atMobile.globals, locale: "ru" },
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSearchUsable(canvasElement, ru.pages.users.searchPlaceholder);
  },
};

let invite: Recorder;
export const InvitesAUser: Story = {
  render: () => {
    invite = recording(
      scoped(async (input, init) => {
        const url = String(input);
        // the default method is an invitation link, so the screen calls
        // createInvitation and reads `accept_url` off the response
        if (init?.method === "POST") {
          return json({ id: "inv-1", accept_url: "https://rolter.local/invite/one-time" }, 201);
        }
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      }),
    );
    return (
      <Harness fetchStub={invite.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Email"), "newcomer@example.com");
    // untouched, the scope is the org itself and the role field says so
    await expect(within(form).getByLabelText("Org role")).toBeInTheDocument();
    await userEvent.click(within(form).getByRole("button", { name: "Invite" }));
    const body = await invite.expectSentBody("POST", "/orgs/org-1/invitations");
    await expect(body).toEqual({
      email: "newcomer@example.com",
      role: "member",
      scope_type: "org",
      scope_id: "org-1",
    });
    // the one-time link is shown once and never again; losing it means the
    // invited person can never accept
    await expect(
      await within(document.body).findByText("https://rolter.local/invite/one-time"),
    ).toBeInTheDocument();
  },
};

/** Invite `newcomer@example.com` and answer the create call with `mail` on the invitation row. */
function invitingWithMail(mail: {
  mail_status: "not_configured" | "sent" | "failed";
  mail_error?: string;
}): Story {
  return {
    render: () => (
      <Harness
        fetchStub={scoped(async (input, init) => {
          const url = String(input);
          if (init?.method === "POST") {
            return json(
              {
                invitation: { id: "inv-1", mail_error: null, ...mail },
                token: "one-time",
                accept_url: "https://rolter.local/invite/one-time",
              },
              201,
            );
          }
          if (url.includes("/invitations")) return json([]);
          return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
        })}
      >
        <Users />
      </Harness>
    ),
    play: async ({ canvasElement }) => {
      await clickWhenEnabled(canvasElement, /invite user/i);
      const form = sheet();
      await userEvent.type(within(form).getByLabelText("Email"), "newcomer@example.com");
      await userEvent.click(within(form).getByRole("button", { name: "Invite" }));
      // the link is the fallback whatever happened to the email
      await expect(
        await within(document.body).findByText("https://rolter.local/invite/one-time"),
      ).toBeInTheDocument();
    },
  };
}

/** With a mail relay the dialog says the link was also emailed (#1828). */
export const InviteLinkSaysItWasEmailed: Story = {
  ...invitingWithMail({ mail_status: "sent" }),
  play: async (context) => {
    await invitingWithMail({ mail_status: "sent" }).play?.(context);
    await expect(
      await within(document.body).findByText(/also emailed this link to newcomer@example.com/),
    ).toBeInTheDocument();
  },
};

/** No relay: the dialog says why nothing was sent, so the admin knows the link is the only copy. */
export const InviteLinkSaysWhyNothingWasEmailed: Story = {
  ...invitingWithMail({ mail_status: "not_configured" }),
  play: async (context) => {
    await invitingWithMail({ mail_status: "not_configured" }).play?.(context);
    await expect(
      await within(document.body).findByText(/no mail relay configured \(ROLTER_SMTP_URL\)/),
    ).toBeInTheDocument();
  },
};

/** A relay that refused: the reason is on screen and the invitation still exists. */
export const InviteLinkSaysTheEmailFailed: Story = {
  ...invitingWithMail({
    mail_status: "failed",
    mail_error: "the mail server could not be reached",
  }),
  play: async (context) => {
    await invitingWithMail({ mail_status: "failed" }).play?.(context);
    await expect(
      await within(document.body).findByText(
        /could not email it: the mail server could not be reached/,
      ),
    ).toBeInTheDocument();
  },
};

/**
 * The invite sheet explains the three choices whose effect is not obvious from
 * the control (#487): how the person gets in, where the role applies, and what
 * each role can do. The same scope and role notes sit on the grant sheet and
 * the change-role dialog.
 */
function explainsTheInviteFields(locale: "en" | "ru"): Story {
  const catalog = locale === "ru" ? ru : en;
  const u = catalog.pages.users;
  const about = (label: string) => catalog.common.aboutField.replace("{{label}}", label);
  return {
    ...(locale === "ru" ? { globals: { locale: "ru" } } : {}),
    render: () => (
      <Harness fetchStub={loaded}>
        <Users />
      </Harness>
    ),
    play: async ({ canvasElement }) => {
      await clickWhenEnabled(canvasElement, u.inviteAction);
      const form = sheet();
      await expectFieldHint(form, about(u.method), u.methodInfo);
      await expectFieldHint(form, about(u.scope), u.scopeInfo);
      await expectFieldHint(form, about(u.orgRole), u.roleInfo);
    },
  };
}
export const ExplainsTheInviteFieldsInAHint: Story = explainsTheInviteFields("en");
export const ExplainsTheInviteFieldsInAHintInRussian: Story = explainsTheInviteFields("ru");

export const ExplainsTheNewRoleInAHint: Story = {
  render: () => (
    <Harness fetchStub={directory()}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(await within(canvasElement).findByRole("button", { name: CHANGE_GRACE }));
    const dialog = await confirmation();
    await expectFieldHint(dialog, "About New role", en.pages.users.roleInfo);
  },
};

/**
 * The invite is refused (#1607).
 *
 * The sheet has to survive the refusal — closing it would drop the address and
 * the role the operator picked — and the refusal has to reach the toast queue,
 * which is the only place this screen reports a rejected write.
 */
export const InviteRejectedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST") {
          return json({ error: { message: "that address already has an invitation" } }, 409);
        }
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Toasted>
        <Users />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Email"), "newcomer@example.com");
    await userEvent.click(within(form).getByRole("button", { name: "Invite" }));
    await expectToast(canvasElement, /already has an invitation/, "error");
    await waitFor(() => expect(within(document.body).getByRole("dialog")).toBeInTheDocument());
    await expect(within(form).getByLabelText("Email")).toHaveValue("newcomer@example.com");
  },
};

/** `Invite user` seeds role=member and method=link, so an untouched form is clean. */
export const AnUntouchedInviteFormClosesWithoutPrompting: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    await expectClosesWithoutPrompting();
  },
};

export const AnEditedInviteFormPromptsBeforeDiscarding: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = sheet();
    await userEvent.type(within(form).getByLabelText("Email"), "half@typed");

    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    await answerDiscardPrompt(false);
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();

    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    await answerDiscardPrompt(true);
    await expectSheetClosed();
  },
};

/** Switching method to `password` reveals the password field and marks dirty. */
export const ChoosingAPasswordRevealsTheField: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = sheet();
    await expect(within(form).queryByLabelText("Password (optional)")).not.toBeInTheDocument();
    await pickOption(within(form).getByLabelText("Method"), "Set a password now");
    await expect(within(form).getByLabelText("Password (optional)")).toBeInTheDocument();
  },
};

// The two gates here take different authorities (#1606).
//
// Inviting someone is `invitation:create`, which is admin, but editing an
// account — the name, the active flag — is `user:update`, which the table
// marks superadmin-only because an account is deployment-wide and not the
// org's to rewrite. So an admin is offered the invite and refused the edit.
export const RefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="viewer">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Invite user");
    await expectRefused(canvasElement, "Grant a role to ada@example.com");
    await expectRefused(canvasElement, REVOKE_GRACE);
    await expectRefused(canvasElement, CHANGE_GRACE);
    await expectRefused(canvasElement, "Edit ada@example.com", NEEDS_SUPERADMIN);
    await expectRefused(canvasElement, "Deactivate ada@example.com", NEEDS_SUPERADMIN);
    await expectRefused(canvasElement, "Reactivate former@example.com", NEEDS_SUPERADMIN);
  },
};

// the org's enrolment roster (#3264): ada has an armed factor, grace has none,
// and the deactivated account is not an active member, so it is not listed
const FACTOR_ROSTER = [
  {
    user_id: "user-1",
    email: "ada@example.com",
    is_superadmin: true,
    enabled: true,
    recovery_codes_remaining: 8,
  },
  {
    user_id: "user-2",
    email: "grace@example.com",
    is_superadmin: false,
    enabled: false,
    recovery_codes_remaining: 0,
  },
];

const withRoster = (answer: () => Response | Promise<Response>): FetchStub => {
  const inner = directory();
  return scoped(async (input, init) =>
    String(input).includes("/mfa-status") ? answer() : inner(input, init),
  );
};

const factorRow = async (canvasElement: HTMLElement, email: string): Promise<HTMLElement> => {
  const row = (await within(canvasElement).findByText(email)).closest('[role="row"]');
  if (!row) throw new Error(`${email} is not rendered as a row`);
  return row as HTMLElement;
};

/** Armed or not set up, found by the column's name and read per row. */
export const ShowsWhoHasASecondFactor: Story = {
  render: () => (
    <Harness fetchStub={withRoster(() => json(FACTOR_ROSTER))} role="admin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("columnheader", { name: "Second factor" })).toBeVisible();
    await waitFor(() =>
      expect(
        within(canvasElement.querySelector('[role="table"]') as HTMLElement).getAllByText("Armed"),
      ).toHaveLength(1),
    );
    await expect(
      within(await factorRow(canvasElement, "ada@example.com")).getByText("Armed"),
    ).toBeVisible();
    await expect(
      within(await factorRow(canvasElement, "grace@example.com")).getByText("Not set up"),
    ).toBeVisible();
    // an account the roster does not list is not claimed to lack a factor
    await expect(
      within(await factorRow(canvasElement, "former@example.com")).getByText("Not listed"),
    ).toBeVisible();
  },
};

export const SecondFactorStillLoading: Story = {
  render: () => (
    <Harness fetchStub={withRoster(() => new Promise<Response>(() => {}))} role="admin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expect(
      await within(canvasElement).findByRole("columnheader", { name: "Second factor" }),
    ).toBeVisible();
    const row = await factorRow(canvasElement, "ada@example.com");
    // no verdict is drawn for a roster that has not arrived
    await expect(within(row).queryByText("Armed")).toBeNull();
    await expect(within(row).queryByText("Not set up")).toBeNull();
  },
};

export const SecondFactorFailedToLoad: Story = {
  render: () => (
    <Harness
      fetchStub={withRoster(() => json({ error: { message: "roster store is down" } }, 500))}
      role="admin"
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /roster store is down/);
    const row = await factorRow(canvasElement, "ada@example.com");
    // a failed roster is not read as "nobody has a factor"
    await expect(within(row).queryByText("Not set up")).toBeNull();
  },
};

/** Below admin the endpoint is refused, so the column is not drawn at all. */
export const SecondFactorHiddenBelowAdmin: Story = {
  render: () => (
    <Harness fetchStub={withRoster(() => json(FACTOR_ROSTER))} role="member">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expect(await within(canvasElement).findByText("ada@example.com")).toBeVisible();
    await expectGateAnswered();
    await expect(
      within(canvasElement).queryByRole("columnheader", { name: "Second factor" }),
    ).toBeNull();
  },
};

// deployment roles (#3290): the list shows each account's role and a superadmin
// sets or clears it with its own endpoint, never as a field of the account edit
const withDeploymentRole = (record?: Recorder): FetchStub => {
  const inner = directory();
  const base: FetchStub = scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    if (method === "PUT" && url.includes("/deployment-role")) {
      const { role } = JSON.parse(String(init?.body)) as { role: string | null };
      return json({ ...USERS[1], deployment_role: role });
    }
    if (method === "GET" && url.includes("/users") && !url.includes("/memberships")) {
      const res = await inner(input, init);
      const rows = (await res.json()) as UserRow[];
      return json(
        rows.map((u) => (u.id === "user-2" ? { ...u, deployment_role: "security_auditor" } : u)),
      );
    }
    return inner(input, init);
  });
  return record ? record.stub : base;
};

export const ShowsTheDeploymentRole: Story = {
  render: () => (
    <Harness fetchStub={withDeploymentRole()} role="superadmin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const row = await factorRow(canvasElement, "grace@example.com");
    await expect(within(row).getByText("Security auditor")).toBeVisible();
    // an account without one carries no badge
    const ada = await factorRow(canvasElement, "ada@example.com");
    await expect(within(ada).queryByText("Security auditor")).toBeNull();
    await expect(within(ada).queryByText("Operator")).toBeNull();
  },
};

const setsDeploymentRole = recording(
  scoped(async (input, init) => withDeploymentRole()(input, init)),
);

export const SetsADeploymentRole: Story = {
  render: () => (
    <Harness fetchStub={setsDeploymentRole.stub} role="superadmin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", {
        name: "Set the deployment role of ada@example.com",
      }),
    );
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await pickOption(panel.getByLabelText("Deployment role"), /^Operator/);
    await userEvent.click(panel.getByRole("button", { name: "Save" }));
    const body = await setsDeploymentRole.expectSentBody<{ role: string | null }>(
      "PUT",
      "/api/v1/users/user-1/deployment-role",
    );
    await expect(body.role).toBe("operator");
  },
};

export const ClearsADeploymentRole: Story = {
  render: () => (
    <Harness fetchStub={setsDeploymentRole.stub} role="superadmin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", {
        name: "Set the deployment role of grace@example.com",
      }),
    );
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    // opens on the role the account holds, and nothing to save until it moves
    await expect(panel.getByLabelText("Deployment role")).toHaveValue("Security auditor");
    await expect(panel.getByRole("button", { name: "Save" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await pickOption(panel.getByLabelText("Deployment role"), /^None/);
    await userEvent.click(panel.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      const sent = setsDeploymentRole.calls.filter(
        (c) => c.method === "PUT" && c.url.includes("/users/user-2/deployment-role"),
      );
      expect(sent.map((c) => JSON.parse(c.body ?? "{}").role)).toContain(null);
    });
  },
};

export const DeploymentRoleRefusedToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={withDeploymentRole()} role="admin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(
      canvasElement,
      "Set the deployment role of ada@example.com",
      NEEDS_SUPERADMIN,
    );
  },
};

export const DeploymentRoleAllowedToASuperadmin: Story = {
  render: () => (
    <Harness fetchStub={withDeploymentRole()} role="superadmin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Set the deployment role of ada@example.com");
  },
};

export const DeploymentRoleRefusedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) =>
        (init?.method ?? "GET").toUpperCase() === "PUT" &&
        String(input).includes("/deployment-role")
          ? json(
              {
                error: {
                  message: "role must be one of operator, security_auditor, null",
                  code: "invalid_field",
                  field: "role",
                },
              },
              400,
            )
          : directory()(input, init),
      )}
      role="superadmin"
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await userEvent.click(
      await within(canvasElement).findByRole("button", {
        name: "Set the deployment role of ada@example.com",
      }),
    );
    await waitFor(() => expect(sheet()).toBeVisible());
    const panel = within(sheet());
    await pickOption(panel.getByLabelText("Deployment role"), /^Operator/);
    await userEvent.click(panel.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(
        panel.getAllByRole("alert").some((a) => /role must be one of/.test(a.textContent ?? "")),
      ).toBe(true),
    );
  },
};

export const EditRefusedToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={loaded} role="admin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Edit ada@example.com", NEEDS_SUPERADMIN);
    await expectRefused(canvasElement, "Deactivate grace@example.com", NEEDS_SUPERADMIN);
    await expectRefused(canvasElement, "Reactivate former@example.com", NEEDS_SUPERADMIN);
    // the invitation half of the screen is still theirs, asserted through
    // `expectAllowed` so it is the gate's answer being read and not the
    // enabled state the button was in before it (#1707)
    await expectAllowed(canvasElement, "Invite user");
    // granting, changing and revoking a role are the org admin's own (#2053)
    await expectAllowed(canvasElement, REVOKE_GRACE);
    await expectAllowed(canvasElement, CHANGE_GRACE);
  },
};

// ------------------------------------------------ revoking and changing a role (#2053)

/**
 * The org as the control plane holds it, for the stories that change it: a
 * revoke takes the grant out of the list, a grant adds one, and the users list
 * is everyone still holding a role in the org, the way `list_in_org` builds it.
 * `onDelete` and `onPost` answer a write before the fixture changes, so a
 * story can hold one in flight or refuse it.
 */
function directory(
  answers: {
    onDelete?: (id: string) => Promise<Response | null> | Response | null;
    onPost?: () => Promise<Response | null> | Response | null;
    onPut?: (id: string) => Promise<Response | null> | Response | null;
    /** grants held on top of `MEMBERSHIPS` */
    extra?: MembershipRow[];
  } = {},
): FetchStub {
  let grants = [...MEMBERSHIPS, ...(answers.extra ?? [])];
  let created = 0;
  return scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    const revoked = url.match(/\/api\/v1\/memberships\/([^/?]+)$/);
    if (method === "DELETE" && revoked) {
      const refused = await answers.onDelete?.(revoked[1]);
      if (refused) return refused;
      grants = grants.filter((g) => g.id !== revoked[1]);
      return json(null, 204);
    }
    if (method === "PUT" && revoked) {
      const refused = await answers.onPut?.(revoked[1]);
      if (refused) return refused;
      const grant = grants.find((g) => g.id === revoked[1]);
      if (!grant) return json({ error: { message: "not found" } }, 404);
      const { role } = JSON.parse(String(init?.body)) as { role: string };
      const updated = { ...grant, role };
      grants = grants.map((g) => (g.id === grant.id ? updated : g));
      return json(updated);
    }
    if (method === "POST" && url.includes("/memberships")) {
      const refused = await answers.onPost?.();
      if (refused) return refused;
      const body = JSON.parse(String(init?.body)) as {
        user_id: string;
        scope_type: string;
        scope_id: string;
        role: string;
      };
      const grant: MembershipRow = {
        id: `m-new-${++created}`,
        user_id: body.user_id,
        org_id: body.scope_type === "org" ? body.scope_id : null,
        team_id: body.scope_type === "team" ? body.scope_id : null,
        project_id: body.scope_type === "project" ? body.scope_id : null,
        role: body.role,
        created_at: "2026-09-30T00:00:00Z",
      };
      grants = [...grants, grant];
      return json(grant);
    }
    if (url.includes("/memberships")) return json(grants);
    if (url.includes("/users")) {
      return json(USERS.filter((u) => grants.some((g) => g.user_id === u.id)));
    }
    return json([]);
  });
}

/**
 * Revoke goes through `ConfirmDialog`: the title names the person, the role
 * and the scope, the body says what they lose. A cancel sends nothing and is
 * an abandon; a confirm is on the wire with both buttons out of reach until
 * it lands, then the dialog closes, the toast says what went, and a person
 * whose last role it was leaves the list.
 */
let revokes: Recorder;
let releaseRevoke: () => void = () => {};
export const RevokeIsConfirmedThenLands: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    revokes = recording(
      directory({
        onDelete: () =>
          new Promise<null>((resolve) => {
            releaseRevoke = () => resolve(null);
          }),
      }),
    );
    return (
      <Harness fetchStub={revokes.stub}>
        <UxScreenProvider screen="gov-users">
          <Toasted>
            <Users />
          </Toasted>
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: REVOKE_GRACE }));
    await expect(
      await within(document.body).findByRole("heading", { name: `${REVOKE_GRACE}?` }),
    ).toBeInTheDocument();
    await cancelConfirmation();
    revokes.expectNotSent("DELETE", "/memberships/");
    const abandon = await expectUxEvent("form_abandon", "user-role-revoke");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "user-role-revoke");

    await userEvent.click(canvas.getByRole("button", { name: REVOKE_GRACE }));
    // what they lose: nothing else reaches the team, and it is their last
    // role in the org, so they leave this list
    await confirmDestructive(
      /no other grant of theirs reaches it\. It is their last role in this organization/,
      "Revoke role",
    );
    await revokes.expectSent("DELETE", "/memberships/m-2");

    // in flight: the request is on the wire, so neither button can be pressed
    const dialog = within(await confirmation());
    await waitFor(() => expect(dialog.getByRole("button", { name: "Revoke role" })).toBeDisabled());
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeDisabled();

    releaseRevoke();
    await expectSheetClosed();
    await expectToast(canvasElement, /Revoked Member on the Platform team from grace@example\.com/);
    await waitFor(() => expect(canvas.queryByText("grace@example.com")).toBeNull());
    const submit = await expectUxEvent("form_submit", "user-role-revoke");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "user-role-revoke");
  },
};

/** the fixture's first invitation, mailed once and not delivered (#3304) */
const withFailedMail = (rows: Invitation[]): Invitation[] =>
  rows.map((row) =>
    row.id === "inv-1"
      ? {
          ...row,
          mail_status: "failed" as const,
          mail_attempted_at: "2026-09-25T00:00:00Z",
          mail_error: "the mail server could not be reached",
        }
      : row,
  );

/**
 * A pending invitation whose email failed can be mailed again from its row. The
 * request names the invitation, the row says it was emailed once the relay
 * took it, and a row that never went through a relay offers no resend at all.
 */
let resendInvite: Recorder;
export const ResendingAnInvitationMailsItAgain: Story = {
  render: () => {
    resendInvite = recording(invitationsApi({ rows: withFailedMail(INVITATIONS) }));
    return (
      <Harness fetchStub={resendInvite.stub}>
        <Toasted>
          <Users />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    const resend = "Email the link to newcomer@example.com again";
    await section.findByRole("button", { name: resend });
    // lead@example.com was never mailed, so there is no link to send again
    await expect(
      section.queryByRole("button", { name: "Email the link to lead@example.com again" }),
    ).toBeNull();
    await expect(await section.findByText("Not emailed")).toBeVisible();

    await userEvent.click(section.getByRole("button", { name: resend }));
    await resendInvite.expectSent("POST", "/invitations/inv-1/resend");
    await expectToast(canvasElement, /Emailed the link to newcomer@example\.com again/);
    await waitFor(() => expect(section.getByText("Emailed")).toBeVisible());
  },
};

/**
 * Check mail relay (#3377): the action sits next to Invite user, sends the
 * check for the org and says what came back in the dashboard's language.
 */
let mailCheck: Recorder;
export const CheckingTheMailRelaySaysItIsFine: Story = {
  render: () => {
    mailCheck = recording(invitationsApi());
    return (
      <Harness fetchStub={mailCheck.stub}>
        <Toasted>
          <Users />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Check mail relay");
    await mailCheck.expectSent("POST", "/invitations/mail-check");
    await expectToast(canvasElement, /mail relay accepted the connection and the credential/);
  },
};

/** each stage that can fail is named, with the reason the API returns */
const failedStage = (stage: "connect" | "tls" | "auth" | "relay", said: string): Story => ({
  render: () => (
    <Harness
      fetchStub={invitationsApi({
        onMailCheck: () => json({ ok: false, stage, reason: `reason for ${stage}` }),
      })}
    >
      <Toasted>
        <Users />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Check mail relay");
    await expectToast(canvasElement, new RegExp(`relay check failed at ${said}`), "error");
    await expect(await within(canvasElement).findByText(`reason for ${stage}`)).toBeVisible();
  },
});

export const MailRelayCheckFailsConnecting: Story = failedStage("connect", "connecting");
export const MailRelayCheckFailsAtTls: Story = failedStage("tls", "setting up TLS");
export const MailRelayCheckFailsSigningIn: Story = failedStage("auth", "signing in");
export const MailRelayCheckFailsAtTheMailServer: Story = failedStage(
  "relay",
  "talking to the mail server",
);

/** a control plane with no relay is stated as such, not as an error */
export const MailRelayCheckWithNoRelayIsNotAnError: Story = {
  render: () => (
    <Harness
      fetchStub={invitationsApi({
        onMailCheck: () =>
          json({ error: { message: "invitation mail is not configured (ROLTER_SMTP_URL)" } }, 409),
      })}
    >
      <Toasted>
        <Users />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Check mail relay");
    await expectToast(canvasElement, /Mail is not configured on this control plane/);
    // a status, not an alert: the alert region stays empty
    const alerts = within(canvasElement).queryAllByRole("alert");
    await expect(alerts.filter((node) => node.textContent)).toHaveLength(0);
  },
};

/** a check run again within a few seconds is told to wait (#3394) */
export const MailRelayCheckRunTooSoonSaysToWait: Story = {
  render: () => (
    <Harness
      fetchStub={invitationsApi({
        onMailCheck: () =>
          new Response(
            JSON.stringify({ error: { message: "too many", code: "too_many_attempts" } }),
            {
              status: 429,
              headers: { "content-type": "application/json", "retry-after": "5" },
            },
          ),
      })}
    >
      <Toasted>
        <Users />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Check mail relay");
    await expectToast(canvasElement, /checked a moment ago/);
  },
};

/** the check is in flight: the button says so and cannot be pressed again */
export const MailRelayCheckInFlight: Story = {
  render: () => (
    <Harness fetchStub={invitationsApi({ onMailCheck: () => new Promise<Response>(() => {}) })}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Check mail relay");
    const busy = await within(canvasElement).findByRole("button", { name: "Checking mail relay" });
    await expect(busy).toBeDisabled();
  },
};

/** the relay is the invitation bar's: a viewer is refused the check */
export const MailRelayCheckRefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={invitationsApi()} role="viewer">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Check mail relay");
  },
};

/** The relay still refuses: the toast says so, with the reason the row keeps. */
export const ResendingAnInvitationThatFailsAgain: Story = {
  render: () => (
    <Harness
      fetchStub={invitationsApi({
        rows: withFailedMail(INVITATIONS),
        onResend: (row) => json({ ...row, mail_error: "the mail server rejected the message" }),
      })}
    >
      <Toasted>
        <Users />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    await userEvent.click(
      await section.findByRole("button", { name: "Email the link to newcomer@example.com again" }),
    );
    await expectToast(
      canvasElement,
      /The link to newcomer@example\.com could not be emailed/,
      "error",
    );
    await expectToast(canvasElement, /the mail server rejected the message/, "error");
  },
};

/**
 * A grant an LDAP sign-in wrote is marked like a single sign-on or SCIM one
 * (#3261), and revoking or changing it says the directory's group mapping will
 * grant it again.
 */
const viaLdap = (): FetchStub => {
  const inner = directory();
  return scoped(async (input, init) => {
    const res = await inner(input, init);
    const method = (init?.method ?? "GET").toUpperCase();
    if (method !== "GET" || !String(input).includes("/memberships")) return res;
    const rows = (await res.json()) as MembershipRow[];
    return json(rows.map((g) => (g.id === "m-2" ? { ...g, source: "ldap" } : g)));
  });
};

export const LdapGrantsAreMarkedAndExplained: Story = {
  render: () => (
    <Harness fetchStub={viaLdap()}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("from LDAP")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: CHANGE_GRACE }));
    await expect(
      await within(document.body).findByText(/came from an LDAP group mapping/),
    ).toBeVisible();
    await cancelConfirmation();
    await userEvent.click(canvas.getByRole("button", { name: REVOKE_GRACE }));
    await expect(
      await within(document.body).findByText(/The role came from an LDAP group mapping/),
    ).toBeVisible();
  },
};

/**
 * The control plane refuses the revoke. The dialog stays open with its message
 * verbatim and a line on what revoking at that scope takes: the gate answered
 * for the scope the switcher is on, which is not always the grant's.
 */
export const RevokeRefusedByTheServer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={directory({
        extra: [GRACE_ORG_ADMIN],
        onDelete: () => json({ error: { message: "requires admin at this scope" } }, 403),
      })}
    >
      <UxScreenProvider screen="gov-users">
        <Users />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", {
        name: "Revoke Admin on the whole organization from ada@example.com",
      }),
    );
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Revoke role" }));
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent("requires admin at this scope"),
    );
    await waitFor(() =>
      expect(
        dialog.getByText(
          "Revoking a role on the whole organization takes Admin there or on a scope above it.",
        ),
      ).toBeVisible(),
    );
    await waitFor(() =>
      expect(
        uxEvents()
          .filter((e) => e.action === "form_submit" && e.target === "user-role-revoke")
          .map((e) => e.outcome),
      ).toEqual(["ok", "error"]),
    );
    expectNoUxEvent("save_confirmed", "user-role-revoke");
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(canvas.getByText("ada@example.com")).toBeInTheDocument();
  },
};

/**
 * A role changes in one confirmed step and one request: `PUT /memberships/{id}`
 * with the new role, no grant and no revoke. The control plane updates the row
 * in a single statement, so the person is never without a role and never holds
 * two.
 */
let changes: Recorder;
export const ChangesARoleInOneRequest: Story = {
  render: () => {
    changes = recording(directory());
    return (
      <Harness fetchStub={changes.stub}>
        <Toasted>
          <Users />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: CHANGE_GRACE }));
    const dialog = within(await confirmation());
    await expect(
      dialog.getByRole("heading", {
        name: "Change grace@example.com's role on the Platform team?",
      }),
    ).toBeInTheDocument();
    // story-wait-allow: disabled by its own confirmDisabled until a role is picked
    await expect(dialog.getByRole("button", { name: "Change role" })).toBeDisabled();

    // the role they already hold is not offered as the one to change to
    const list = await openOptions(dialog.getByLabelText("New role"));
    await expect(within(list).queryByRole("option", { name: "Member" })).toBeNull();
    await userEvent.click(within(list).getByRole("option", { name: "Viewer" }));
    await userEvent.click(dialog.getByRole("button", { name: "Change role" }));

    const body = await changes.expectSentBody("PUT", "/memberships/m-2");
    await expect(body).toEqual({ role: "viewer" });
    await expect(changes.calls.filter((c) => c.method !== "GET").map((c) => c.method)).toEqual([
      "PUT",
    ]);
    await expectSheetClosed();
    await expectToast(canvasElement, /grace@example\.com is now Viewer on the Platform team/);
    await waitFor(() =>
      expect(
        canvas.getByRole("button", {
          name: "Revoke Viewer on the Platform team from grace@example.com",
        }),
      ).toBeInTheDocument(),
    );
  },
};

/**
 * The change is refused: the dialog stays open with the control plane's
 * message, the row keeps its role, and the same confirm sends it again. There
 * is no half-applied state to explain.
 */
let refusedChange: Recorder;
let failedChanges = 0;
export const ChangeThatFailsLeavesTheRoleAsItWas: Story = {
  render: () => {
    failedChanges = 0;
    refusedChange = recording(
      directory({
        onPut: () =>
          failedChanges++ === 0 ? json({ error: { message: "store unavailable" } }, 500) : null,
      }),
    );
    return (
      <Harness fetchStub={refusedChange.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: CHANGE_GRACE }));
    const dialog = within(await confirmation());
    await pickOption(dialog.getByLabelText("New role"), "Viewer");
    await userEvent.click(dialog.getByRole("button", { name: "Change role" }));

    await waitFor(() => expect(dialog.getByRole("alert")).toHaveTextContent("store unavailable"));
    await expect(canvas.getByRole("button", { name: CHANGE_GRACE })).toBeInTheDocument();

    await userEvent.click(dialog.getByRole("button", { name: "Change role" }));
    await expectSheetClosed();
    await expect(refusedChange.calls.filter((c) => c.method === "PUT")).toHaveLength(2);
    await expect(refusedChange.calls.filter((c) => c.method === "POST")).toHaveLength(0);
    await expect(refusedChange.calls.filter((c) => c.method === "DELETE")).toHaveLength(0);
  },
};

/**
 * A person who already holds the picked role at that scope through another
 * grant has the grant being changed revoked, since updating it would only
 * duplicate the other.
 */
let duplicateChange: Recorder;
export const ChangeToARoleAlreadyHeldRevokesInstead: Story = {
  render: () => {
    duplicateChange = recording(
      directory({
        extra: [{ ...MEMBERSHIPS[1], id: "m-dup", role: "viewer" }],
      }),
    );
    return (
      <Harness fetchStub={duplicateChange.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: CHANGE_GRACE }));
    const dialog = within(await confirmation());
    await pickOption(dialog.getByLabelText("New role"), "Viewer");
    await userEvent.click(dialog.getByRole("button", { name: "Change role" }));
    await duplicateChange.expectSent("DELETE", "/memberships/m-2");
    await expectSheetClosed();
    await expect(duplicateChange.calls.filter((c) => c.method === "PUT")).toHaveLength(0);
  },
};

/**
 * The org's last admin grant is not offered to anyone but a superadmin: the
 * control plane would refuse it (#2311), so both actions on it are disabled
 * and say why. A deactivated admin does not count as another one, and an admin
 * grant on a team is not an org admin at all.
 */
export const LastOrgAdminGrantIsNotOffered: Story = {
  render: () => (
    <Harness
      fetchStub={directory({
        extra: [
          { ...GRACE_ORG_ADMIN, id: "m-8", user_id: "user-3" },
          { ...GRACE_ORG_ADMIN, id: "m-7", org_id: null, team_id: "team-1" },
        ],
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const revoke = await canvas.findByRole("button", { name: REVOKE_ADA });
    // story-wait-allow: disabled by its own prop from the first paint
    await expect(revoke).toBeDisabled();
    // story-wait-allow: disabled by its own prop from the first paint
    await expect(canvas.getByRole("button", { name: CHANGE_ADA })).toBeDisabled();
    const reason =
      "ada@example.com's Admin role on the whole organization is the organization's last admin grant. Grant admin to someone else first.";
    await expect(revoke).toHaveAttribute("title", reason);
    // the other grants keep their controls
    await expect(canvas.getByRole("button", { name: REVOKE_GRACE })).toBeEnabled();
  },
};

/** Another active admin makes the grant revocable again. */
export const AnotherAdminLiftsTheLastAdminHold: Story = {
  render: () => (
    <Harness fetchStub={directory({ extra: [GRACE_ORG_ADMIN] })}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("button", { name: REVOKE_ADA })).toBeEnabled();
    await expect(canvas.getByRole("button", { name: CHANGE_ADA })).toBeEnabled();
  },
};

/** A superadmin is exempt on the server, so the dashboard offers the action. */
export const SuperadminMayRevokeTheLastAdminGrant: Story = {
  render: () => (
    <SignedInAs user={USERS[0]}>
      <Harness fetchStub={directory()}>
        <Users />
      </Harness>
    </SignedInAs>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("button", { name: REVOKE_ADA })).toBeEnabled();
  },
};

/**
 * The list was stale: someone else's admin grant went away meanwhile, so the
 * control plane refuses with `last_org_admin`. The dialog stays open and says
 * why in the catalog's sentence for the code, the shared path every coded
 * refusal takes (#3522), branching on the code and not the message.
 */
export const RevokeRefusedAsTheLastAdmin: Story = {
  render: () => (
    <Harness fetchStub={directory({ extra: [GRACE_ORG_ADMIN], onDelete: lastAdminRefusal })}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: REVOKE_ADA }));
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Revoke role" }));
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent(en.errors.api.codes.last_org_admin),
    );
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
  },
};

/**
 * Demoting the last admin grant is refused with the stable `last_org_admin`
 * code: the dialog stays open and says why in the catalog's sentence for the
 * code (#3522), and the row keeps its role.
 */
export const ChangeOfTheLastAdminIsRefused: Story = {
  render: () => (
    <Harness fetchStub={directory({ extra: [GRACE_ORG_ADMIN], onPut: lastAdminRefusal })}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: CHANGE_ADA }));
    const dialog = within(await confirmation());
    await pickOption(dialog.getByLabelText("New role"), "Member");
    await userEvent.click(dialog.getByRole("button", { name: "Change role" }));
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent(en.errors.api.codes.last_org_admin),
    );
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(canvas.getByRole("button", { name: CHANGE_ADA })).toBeInTheDocument();
  },
};

/**
 * The grant sheet picks its scope by name from the shared `OrgScopePicker`,
 * where it used to ask for a pasted project uuid, and posts the project it
 * names.
 */
let grantsSent: Recorder;
export const GrantsAProjectRolePickedByName: Story = {
  render: () => {
    grantsSent = recording(directory());
    return (
      <Harness fetchStub={grantsSent.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Grant a role to grace@example.com");
    const form = within(sheet());
    await pickOption(await form.findByLabelText("Scope"), "Gateway");
    await pickOption(form.getByLabelText("Role"), "Viewer");
    await expect(form.queryByPlaceholderText(/0000/)).toBeNull();
    await userEvent.click(form.getByRole("button", { name: "Grant" }));
    const body = await grantsSent.expectSentBody("POST", "/orgs/org-1/memberships");
    await expect(body).toEqual({
      user_id: "user-2",
      scope_type: "project",
      scope_id: "project-1",
      role: "viewer",
    });
    await expectSheetClosed();
  },
};

/** A role the person already holds at that scope is refused before any request. */
export const AGrantAlreadyHeldIsRefusedBeforeSending: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, "Grant a role to grace@example.com");
    const form = within(sheet());
    await pickOption(await form.findByLabelText("Scope"), "Platform");
    await waitFor(() =>
      expect(
        form.getByText("grace@example.com already holds Member on the Platform team."),
      ).toBeVisible(),
    );
    // story-wait-allow: disabled by the sheet's own canSave, which the held grant sets
    await expect(form.getByRole("button", { name: "Grant" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

// ------------------------------------------ invitations at a scope, and pending (#2054)

/**
 * The org's invitations as the control plane holds them: the list answers every
 * one, a revoke stamps `revoked_at` and a create adds a row. `onRevoke` answers
 * a revoke first and may change the fixture or refuse, so a story can hold a
 * revoke in flight, refuse it or accept the invitation behind the screen's back.
 */
function invitationsApi(
  answers: {
    rows?: Invitation[];
    onRevoke?: (id: string, rows: Invitation[]) => Promise<Response | null> | Response | null;
    /** answers a resend of the invitation with this id; the default mails it */
    onResend?: (row: Invitation) => Response;
    /** answers a relay check; the default says the relay is fine */
    onMailCheck?: () => Response | Promise<Response>;
  } = {},
): FetchStub {
  const rows = (answers.rows ?? INVITATIONS).map((row) => ({ ...row }));
  return scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    const revoked = url.match(/\/api\/v1\/invitations\/([^/?]+)$/);
    if (method === "DELETE" && revoked) {
      const refused = await answers.onRevoke?.(revoked[1], rows);
      if (refused) return refused;
      const row = rows.find((candidate) => candidate.id === revoked[1]);
      if (!row) return json({ error: { message: "not found" } }, 404);
      row.revoked_at = "2026-09-30T00:00:00Z";
      return json(row);
    }
    if (method === "POST" && url.includes("/invitations/mail-check")) {
      return answers.onMailCheck ? answers.onMailCheck() : json({ ok: true });
    }
    const resent = url.match(/\/api\/v1\/invitations\/([^/?]+)\/resend$/);
    if (method === "POST" && resent) {
      const row = rows.find((candidate) => candidate.id === resent[1]);
      if (!row) return json({ error: { message: "not found" } }, 404);
      if (answers.onResend) return answers.onResend(row);
      row.mail_status = "sent";
      row.mail_error = null;
      return json(row);
    }
    if (method === "POST" && url.includes("/invitations")) {
      return json({ accept_url: "https://rolter.local/invite/one-time" }, 201);
    }
    if (url.includes("/invitations")) return json(rows);
    if (url.includes("/memberships")) return json(MEMBERSHIPS);
    if (url.includes("/users")) return json(USERS);
    return json([]);
  });
}

/** the pending section, found by the heading that names it */
async function pendingSection(canvasElement: HTMLElement): Promise<HTMLElement> {
  return within(canvasElement).findByRole("region", { name: "Pending invitations" });
}

/**
 * The house date, written out the way `useFormat().date` renders it for `en`,
 * so the story asserts the expiry went through the formatter and not through a
 * bare `toLocaleDateString`.
 */
const expiryDate = (iso: string) =>
  new Intl.DateTimeFormat("en", { dateStyle: "medium" }).format(new Date(iso));

/**
 * The invite sheet picks its scope by name from the shared `OrgScopePicker`,
 * and the role field is named for the scope it lands in. The roles read as
 * Admin, Member and Viewer, never as the raw keys. The body sent carries the
 * project the operator picked, and the link dialog says what accepting it
 * grants.
 */
let projectInvite: Recorder;
export const InvitesToAProjectPickedByName: Story = {
  render: () => {
    projectInvite = recording(invitationsApi());
    return (
      <Harness fetchStub={projectInvite.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "engineer@example.com");
    await pickOption(await form.findByLabelText("Scope"), "Gateway");

    // the role field follows the scope: it was "Org role" a moment ago
    await waitFor(() => expect(form.getByLabelText("Project role")).toBeInTheDocument());
    await expect(form.queryByLabelText("Org role")).toBeNull();
    const roles = await openOptions(form.getByLabelText("Project role"));
    await expect(
      within(roles)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["Admin", "Member", "Viewer"]);
    await userEvent.click(within(roles).getByRole("option", { name: "Viewer" }));

    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const body = await projectInvite.expectSentBody("POST", "/orgs/org-1/invitations");
    await expect(body).toEqual({
      email: "engineer@example.com",
      role: "viewer",
      scope_type: "project",
      scope_id: "project-1",
    });

    const dialog = within(await confirmation());
    await waitFor(() =>
      expect(dialog.getByText("https://rolter.local/invite/one-time")).toBeVisible(),
    );
    await waitFor(() =>
      expect(dialog.getByText("Accepting it grants Viewer on the Gateway project.")).toBeVisible(),
    );
  },
};

/** A team is a scope of its own: the body names the team and the field says "Team role". */
let teamInvite: Recorder;
export const InvitesToATeamPickedByName: Story = {
  render: () => {
    teamInvite = recording(invitationsApi());
    return (
      <Harness fetchStub={teamInvite.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "lead@example.com");
    await pickOption(await form.findByLabelText("Scope"), "Platform");
    await waitFor(() => expect(form.getByLabelText("Team role")).toBeInTheDocument());
    await pickOption(form.getByLabelText("Team role"), "Admin");
    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const body = await teamInvite.expectSentBody("POST", "/orgs/org-1/invitations");
    await expect(body).toEqual({
      email: "lead@example.com",
      role: "admin",
      scope_type: "team",
      scope_id: "team-1",
    });
  },
};

/**
 * An account created with a password is granted its role on the org, since the
 * endpoint takes no scope. The picker is locked to the org and says why, and
 * nothing it held before is sent.
 */
let passwordInvite: Recorder;
export const APasswordAccountTakesItsRoleOnTheOrg: Story = {
  render: () => {
    passwordInvite = recording(invitationsApi());
    return (
      <Harness fetchStub={passwordInvite.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "service@example.com");
    await pickOption(await form.findByLabelText("Scope"), "Platform");
    await pickOption(form.getByLabelText("Method"), "Set a password now");

    await waitFor(() =>
      expect(
        form.getByText(
          "An account created with a password takes its role on the whole organization. Invite by link to choose a team or a project.",
        ),
      ).toBeVisible(),
    );
    // story-wait-allow: disabled by the sheet's own method, which the pick above set
    await expect(form.getByLabelText("Scope")).toBeDisabled();
    await expect(form.getByLabelText("Scope")).toHaveValue("Whole organization");
    await expect(form.getByLabelText("Org role")).toBeInTheDocument();

    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const body = await passwordInvite.expectSentBody("POST", "/orgs/org-1/users");
    await expect(body).toEqual({ email: "service@example.com", role: "member" });
  },
};

/**
 * The scope switcher's gate answered for its own scope, so the control plane
 * can still refuse an invitation at the one picked. The sheet stays open with
 * the message verbatim and says what inviting there takes.
 */
export const InviteRefusedAtTheScopePicked: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST") {
          return json({ error: { message: "insufficient role for this resource" } }, 403);
        }
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "newcomer@example.com");
    await pickOption(await form.findByLabelText("Scope"), "Platform");
    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    await waitFor(() =>
      expect(
        form.getByText("Inviting into the Platform team takes Admin there or on a scope above it."),
      ).toBeVisible(),
    );
    await expect(form.getByText("Your role does not permit this.")).toBeVisible();
    await expect(form.getByLabelText("Email")).toHaveValue("newcomer@example.com");
  },
};

/** What the stub clipboard received, so a play can read it back. */
let copied: string[] = [];

/** A clipboard the story owns, put back when it ends. */
function stubClipboard(writeText: (value: string) => Promise<void>) {
  return () => {
    const original = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    return () => {
      if (original) Object.defineProperty(navigator, "clipboard", original);
      else Reflect.deleteProperty(navigator, "clipboard");
    };
  };
}

const INVITE_LINK = "https://rolter.local/invite/one-time";

/** The link dialog copies through the shared `CopyButton`, and what lands is the link itself. */
export const TheInviteLinkCopies: Story = {
  beforeEach: stubClipboard(async (value) => {
    copied = [...copied, value];
  }),
  render: () => (
    <Harness fetchStub={invitationsApi()}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    copied = [];
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "newcomer@example.com");
    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const dialog = within(await confirmation());
    await userEvent.click(await dialog.findByRole("button", { name: /Copy link/ }));
    await waitFor(() => expect(copied).toEqual([INVITE_LINK]));
    await waitFor(() =>
      expect(dialog.getByRole("button", { name: /Copy link/ })).toHaveAttribute(
        "title",
        en.common.copied,
      ),
    );
    // a link that reached the clipboard closes without a question (#2217)
    await userEvent.click(dialog.getByRole("button", { name: "Done" }));
    await expectSheetClosed();
  },
};

/**
 * The clipboard is withheld on a plain-http dashboard, which is common on an
 * air-gapped LAN. The button used to say "Copied" whatever happened; now the
 * dialog says the copy failed in a line that stays (#2327), the link stays on
 * screen and selected, and closing it asks, since nobody has it yet (#2217).
 */
export const TheInviteLinkCopyFails: Story = {
  beforeEach: stubClipboard(() => Promise.reject(new Error("denied"))),
  render: () => (
    <Harness fetchStub={invitationsApi()}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "newcomer@example.com");
    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const dialog = within(await confirmation());
    await waitFor(() => expect(dialog.getByText(INVITE_LINK)).toBeVisible());
    await userEvent.click(dialog.getByRole("button", { name: /Copy link/ }));
    await waitFor(() =>
      expect(dialog.getByRole("button", { name: /Copy link/ })).toHaveAttribute(
        "title",
        en.common.copyFailed,
      ),
    );
    await expect(dialog.queryByText(en.common.copied)).toBeNull();
    await expect(await dialog.findByRole("alert")).toHaveTextContent(en.common.copyFailed);
    // the link is still there to copy by hand, selected for it
    await expect(dialog.getByText(INVITE_LINK)).toBeVisible();
    await expect(window.getSelection()?.toString()).toBe(INVITE_LINK);

    // and only a confirmed close takes it away
    await userEvent.click(dialog.getByRole("button", { name: "Done" }));
    await answerSecretClosePrompt(false);
    await expect(dialog.getByText(INVITE_LINK)).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Done" }));
    await answerSecretClosePrompt(true);
    await expectSheetClosed();
  },
};

/**
 * The invitation link is shown once and cannot be recovered, so Escape, the
 * scrim and the close button ask before closing over a link nobody copied.
 * Cancelling keeps the dialog, the link and what it says to do with it.
 */
export const AnUncopiedInviteLinkAsksBeforeClosing: Story = {
  render: () => (
    <Harness fetchStub={invitationsApi()}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    await userEvent.type(form.getByLabelText("Email"), "newcomer@example.com");
    await userEvent.click(form.getByRole("button", { name: "Invite" }));
    const dialog = within(await confirmation());
    await waitFor(() => expect(dialog.getByText(INVITE_LINK)).toBeVisible());
    // the next step is who to send it to and what it grants
    await expect(dialog.getByText("newcomer@example.com")).toBeVisible();

    await userEvent.keyboard("{Escape}");
    await answerSecretClosePrompt(false);
    await expect(dialog.getByText(INVITE_LINK)).toBeVisible();
    await expect(dialog.getByText(/Accepting it grants/)).toBeVisible();

    await userEvent.click(dialog.getByRole("button", { name: en.common.close }));
    await answerSecretClosePrompt(true);
    await expectSheetClosed();
  },
};

/**
 * The pending section lists what has neither been accepted nor revoked: the
 * address, the role by its translated name, the scope by name, who sent it
 * (the address the control plane resolved, never a uuid) and the expiry through
 * `fmt`. The accepted and the revoked invitation are not listed, and an
 * expired one is, marked, since it still holds its address.
 */
export const PendingInvitationsAreListed: Story = {
  render: () => (
    <Harness fetchStub={invitationsApi()}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const section = within(await pendingSection(canvasElement));
    await expectListTable(canvasElement, "Pending invitations");
    await expect(await section.findByText("newcomer@example.com")).toBeVisible();

    const row = (email: string) =>
      within(section.getByText(email).closest('[role="row"]') as HTMLElement);
    // the sender comes on the row itself (#2325); the scope column is drawn
    // from the org's teams and projects, a skeleton until that read answers
    await expect(row("newcomer@example.com").getByText("ada@example.com")).toBeVisible();
    await expect(await row("newcomer@example.com").findByText("Gateway")).toBeVisible();
    await expect(row("newcomer@example.com").getByText("Member")).toBeVisible();
    await expect(
      row("newcomer@example.com").getByText(expiryDate(INVITATIONS[0].expires_at)),
    ).toBeVisible();
    await expect(row("lead@example.com").getByText("Admin")).toBeVisible();
    await expect(row("lead@example.com").getByText("Platform")).toBeVisible();
    await expect(row("lead@example.com").getByText("grace@example.com")).toBeVisible();
    await expect(row("everyone@example.com").getByText("Viewer")).toBeVisible();
    await expect(row("everyone@example.com").getByText("Whole organization")).toBeVisible();
    await expect(row("everyone@example.com").getByText("not recorded")).toBeVisible();
    // a sender outside the users list is still named, since the control plane
    // resolves the address: it is no longer "unknown"
    await expect(row("stale@example.com").getByText("dana@elsewhere.example")).toBeVisible();
    await expect(row("stale@example.com").getByText("Expired")).toBeVisible();
    await expect(
      row("stale@example.com").getByText(expiryDate("2026-01-10T00:00:00Z")),
    ).toBeVisible();
    await expect(row("newcomer@example.com").queryByText("Expired")).toBeNull();

    await expect(section.queryByText("joined@example.com")).toBeNull();
    await expect(section.queryByText("withdrawn@example.com")).toBeNull();
    await expect(section.queryByText(/user-9|project-1|team-1/)).toBeNull();
    await expect(
      section.getByText("4 not accepted yet · a link works once and expires after seven days"),
    ).toBeVisible();
    // the users table above is a table of its own
    await expectListTable(canvasElement, "Users");
    await expect(canvas.getAllByRole("table")).toHaveLength(2);
  },
};

// the operator `rolter-seed --admin-email` creates: a superadmin who holds no
// role anywhere, so no org's membership list names them (#2804)
const OPERATOR: UserRow = {
  id: "user-operator",
  email: "operator@example.com",
  is_superadmin: true,
  deactivated_at: null,
  created_at: "2026-01-01T00:00:00Z",
};

let operatorReads: Recorder;
/**
 * An invitation a superadmin sent from an account that holds no role anywhere
 * names them. The control plane records the signed-in account whichever kind of
 * principal it is (#2813) and returns its address on the row (#2325). The row
 * shows the address, never "not recorded", which is for the admin token.
 */
export const PendingInvitationSentByASuperadminNamesThem: Story = {
  render: () => {
    const sent: Invitation[] = [
      {
        ...INVITATIONS[0],
        id: "inv-operator",
        email: "from-operator@example.com",
        invited_by: OPERATOR.id,
        invited_by_email: OPERATOR.email,
      },
      INVITATIONS[2],
    ];
    operatorReads = recording(
      scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) return json(sent);
        if (url.includes("/memberships")) return json(MEMBERSHIPS);
        if (url.includes("/users")) {
          return json(url.includes("include_unassigned=true") ? [...USERS, OPERATOR] : USERS);
        }
        return json([]);
      }),
    );
    return (
      <SignedInAs user={OPERATOR}>
        <Harness fetchStub={operatorReads.stub}>
          <Users />
        </Harness>
      </SignedInAs>
    );
  },
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    const row = (email: string) =>
      within(section.getByText(email).closest('[role="row"]') as HTMLElement);
    await expect(await section.findByText("from-operator@example.com")).toBeVisible();
    await expect(await row("from-operator@example.com").findByText(OPERATOR.email)).toBeVisible();
    await expect(row("from-operator@example.com").queryByText("not recorded")).toBeNull();
    await expect(row("from-operator@example.com").queryByText("unknown")).toBeNull();
    // the admin token still names nobody, and says so
    await expect(row("everyone@example.com").getByText("not recorded")).toBeVisible();
    await operatorReads.expectSent("GET", "include_unassigned=true");
  },
};

/** The list is a read of its own: while it is in flight the section is a skeleton, not "no invitations". */
export const PendingInvitationsLoading: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) return new Promise<Response>(() => {});
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const section = await pendingSection(canvasElement);
    await waitFor(() =>
      expect(within(section).getAllByLabelText(LOADING_LABEL).length).toBeGreaterThan(0),
    );
    await expectNoFalseEmpty(section, /No pending invitations/);
  },
};

/** A failed read says so under the section's own `LoadError`, and claims neither none nor a count. */
export const PendingInvitationsFailToLoad: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) {
          return json({ error: { message: "invitation store unavailable" } }, 500);
        }
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /invitation store unavailable/);
    const section = await pendingSection(canvasElement);
    await expectNoFalseEmpty(section, /No pending invitations/);
    // the users list above is unaffected
    await expect(await within(canvasElement).findByText("grace@example.com")).toBeVisible();
  },
};

/**
 * Nothing pending says so, and offers the invite. An accepted and a revoked
 * invitation do not make a pending list: they are spent.
 */
export const NoPendingInvitations: Story = {
  render: () => (
    <Harness fetchStub={invitationsApi({ rows: [INVITATIONS[4], INVITATIONS[5]] })}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const section = await pendingSection(canvasElement);
    await expectEmptyState(section, /No pending invitations/, /Invite user/);
    await expect(within(section).queryByText("joined@example.com")).toBeNull();
    await expect(within(section).queryByText("withdrawn@example.com")).toBeNull();
  },
};

/** The invitations are the org admin's to read: a viewer has no section and no request. */
let viewerReads: Recorder;
export const PendingInvitationsHiddenFromAViewer: Story = {
  render: () => {
    viewerReads = recording(invitationsApi());
    return (
      <Harness fetchStub={viewerReads.stub} role="viewer">
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("grace@example.com")).toBeInTheDocument();
    await expectGateAnswered();
    await expect(canvas.queryByRole("region", { name: "Pending invitations" })).toBeNull();
    viewerReads.expectNotSent("GET", "/invitations");
  },
};

/** An org admin reads the list and may revoke and resend from it. */
export const PendingInvitationsOpenToAnAdmin: Story = {
  render: () => (
    <Harness fetchStub={invitationsApi({ rows: withFailedMail(INVITATIONS) })} role="admin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const section = await pendingSection(canvasElement);
    await expectAllowed(section, revokeInvitationName("newcomer@example.com"));
    // resending is `invitation:create`, the bar for sending it the first time
    await expectAllowed(section, "Email the link to newcomer@example.com again");
  },
};

/**
 * A 403 from the list is an answer, not an outage: a caller the gate could not
 * refuse first (a role held below the org) gets no section instead of a red
 * "you do not have access to pending invitations" on a screen they may open.
 */
export const PendingInvitationsRefusedByTheServer: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) {
          return json({ error: { message: "insufficient role for this resource" } }, 403);
        }
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("grace@example.com")).toBeInTheDocument();
    await waitFor(() =>
      expect(canvas.queryByRole("region", { name: "Pending invitations" })).toBeNull(),
    );
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

/**
 * Revoke goes through `ConfirmDialog`: the title names the address and the body
 * says what the link can no longer do. A cancel sends nothing and is an
 * abandon; a confirm is on the wire with both buttons out of reach until it
 * lands, then the dialog closes, the toast says what went and the row leaves.
 */
let revokeInvite: Recorder;
let releaseInvite: () => void = () => {};
export const RevokingAPendingInvitationIsConfirmed: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    revokeInvite = recording(
      invitationsApi({
        onRevoke: () =>
          new Promise<null>((resolve) => {
            releaseInvite = () => resolve(null);
          }),
      }),
    );
    return (
      <Harness fetchStub={revokeInvite.stub}>
        <UxScreenProvider screen="gov-users">
          <Toasted>
            <Users />
          </Toasted>
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    const revoke = revokeInvitationName("newcomer@example.com");
    await userEvent.click(await section.findByRole("button", { name: revoke }));
    await expect(
      await within(document.body).findByRole("heading", { name: `${revoke}?` }),
    ).toBeInTheDocument();
    await cancelConfirmation();
    revokeInvite.expectNotSent("DELETE", "/invitations/");
    const abandon = await expectUxEvent("form_abandon", "invitation-revoke");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "invitation-revoke");

    await userEvent.click(section.getByRole("button", { name: revoke }));
    await confirmDestructive(
      /The link stops working straight away, so newcomer@example\.com cannot join the Gateway project as Member with it\./,
      "Revoke invitation",
    );
    await revokeInvite.expectSent("DELETE", "/invitations/inv-1");

    // in flight: the request is on the wire, so neither button can be pressed
    const dialog = within(await confirmation());
    await waitFor(() =>
      expect(dialog.getByRole("button", { name: "Revoke invitation" })).toBeDisabled(),
    );
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeDisabled();

    releaseInvite();
    await expectSheetClosed();
    await expectToast(canvasElement, /Revoked the invitation for newcomer@example\.com/);
    await waitFor(() => expect(section.queryByText("newcomer@example.com")).toBeNull());
    await expect(section.getByText("lead@example.com")).toBeVisible();
    const submit = await expectUxEvent("form_submit", "invitation-revoke");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "invitation-revoke");
  },
};

/** A dead link is revoked only to free its address, and the body says so. */
let revokeExpired: Recorder;
export const RevokingAnExpiredInvitationFreesTheAddress: Story = {
  render: () => {
    revokeExpired = recording(invitationsApi());
    return (
      <Harness fetchStub={revokeExpired.stub}>
        <Toasted>
          <Users />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    await userEvent.click(
      await section.findByRole("button", { name: revokeInvitationName("stale@example.com") }),
    );
    await confirmDestructive(
      /This invitation has already expired and its link no longer works\. Revoking it removes it from the list and lets you invite stale@example\.com again\./,
      "Revoke invitation",
    );
    await revokeExpired.expectSent("DELETE", "/invitations/inv-4");
    await expectSheetClosed();
    await waitFor(() => expect(section.queryByText("stale@example.com")).toBeNull());
  },
};

/**
 * The control plane refuses the revoke. The dialog stays open with its message
 * verbatim and a line on what revoking at that scope takes: the gate answered
 * for the scope the switcher is on, which is not always the invitation's.
 */
export const RevokingAnInvitationRefusedByTheServer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={invitationsApi({
        onRevoke: () => json({ error: { message: "insufficient role for this resource" } }, 403),
      })}
    >
      <UxScreenProvider screen="gov-users">
        <Users />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    await userEvent.click(
      await section.findByRole("button", { name: revokeInvitationName("lead@example.com") }),
    );
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Revoke invitation" }));
    await waitFor(() =>
      expect(dialog.getByRole("alert")).toHaveTextContent("insufficient role for this resource"),
    );
    await waitFor(() =>
      expect(
        dialog.getByText(
          "Revoking an invitation into the Platform team takes Admin there or on a scope above it.",
        ),
      ).toBeVisible(),
    );
    expectNoUxEvent("save_confirmed", "invitation-revoke");
    await expect(section.getByText("lead@example.com")).toBeVisible();
  },
};

/**
 * The invitee accepted while the dialog was open. The revoke only touches
 * unaccepted invitations, so it answers 404: the dialog says the invitation is
 * no longer pending, and the list is read again and drops the row.
 */
let revokeAccepted: Recorder;
export const RevokingAnInvitationAcceptedMeanwhile: Story = {
  render: () => {
    revokeAccepted = recording(
      invitationsApi({
        onRevoke: (id, rows) => {
          const row = rows.find((candidate) => candidate.id === id);
          if (row) row.accepted_at = "2026-09-30T00:00:00Z";
          return json({ error: { message: `pending invitation ${id} not found` } }, 404);
        },
      }),
    );
    return (
      <Harness fetchStub={revokeAccepted.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    await userEvent.click(
      await section.findByRole("button", { name: revokeInvitationName("lead@example.com") }),
    );
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Revoke invitation" }));
    await waitFor(() =>
      expect(
        dialog.getByText(
          "This invitation is no longer pending, most likely because it was just accepted. The list has been refreshed.",
        ),
      ).toBeVisible(),
    );
    await waitFor(() => expect(section.queryByText("lead@example.com")).toBeNull());
    await expect(
      revokeAccepted.calls.filter((c) => c.method === "GET" && c.url.includes("/invitations"))
        .length,
    ).toBeGreaterThan(1);
  },
};

// -------------------------- deactivating, deleting and granting superadmin (#2055)

/** the account the caller is signed in as in the stories that need one: ada, a superadmin */
const ME = USERS[0];

/**
 * Signed in the way a login leaves the browser, minus the token, so the
 * provider knows the account without asking /auth/me for it. The screen reads
 * it to say when a change is to the caller's own account.
 */
function SignedInAs({ user, children }: { user: UserRow; children: React.ReactNode }) {
  React.useState(() => {
    localStorage.setItem("rolter.session.email", user.email);
    localStorage.setItem("rolter.session.user", JSON.stringify(user));
    localStorage.removeItem("rolter.session.token");
  });
  React.useEffect(
    () => () => {
      localStorage.removeItem("rolter.session.email");
      localStorage.removeItem("rolter.session.user");
    },
    [],
  );
  return <AuthProvider>{children}</AuthProvider>;
}

/**
 * The accounts as the control plane holds them, for the stories that change
 * one: a PUT applies the fields it names, a DELETE removes the row, and the
 * list answers whatever is left. `onPut` and `onDelete` answer a write before
 * the fixture changes, so a story can hold one in flight, refuse it, or change
 * the fixture behind the screen's back through `state`.
 */
function accountsApi(
  answers: {
    onPut?: (id: string, body: UpdateUserInput) => Promise<Response | null> | Response | null;
    onDelete?: (
      id: string,
      state: { accounts: UserRow[] },
    ) => Promise<Response | null> | Response | null;
  } = {},
): FetchStub {
  const state = { accounts: USERS.map((user) => ({ ...user })) };
  return scoped(async (input, init) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    const account = url.match(/\/api\/v1\/users\/([^/?]+)$/);
    if (account && method === "PUT") {
      const body = JSON.parse(String(init?.body)) as UpdateUserInput;
      const refused = await answers.onPut?.(account[1], body);
      if (refused) return refused;
      const current = state.accounts.find((user) => user.id === account[1]);
      if (!current) return json({ error: { message: "user not found" } }, 404);
      const next: UserRow = {
        ...current,
        email: body.email ?? current.email,
        is_superadmin: body.is_superadmin ?? current.is_superadmin,
        deactivated_at:
          body.deactivated === undefined
            ? current.deactivated_at
            : body.deactivated
              ? "2026-09-30T00:00:00Z"
              : null,
      };
      state.accounts = state.accounts.map((user) => (user.id === next.id ? next : user));
      return json(next);
    }
    if (account && method === "DELETE") {
      const refused = await answers.onDelete?.(account[1], state);
      if (refused) return refused;
      state.accounts = state.accounts.filter((user) => user.id !== account[1]);
      return json(null, 204);
    }
    if (url.includes("/invitations")) return json([]);
    if (url.includes("/memberships")) return json(MEMBERSHIPS);
    if (url.includes("/users")) return json(state.accounts);
    return json([]);
  });
}

/** a write that stays on the wire until the returned function is called */
function held(): { answer: () => Promise<null>; release: () => void } {
  let release: () => void = () => {};
  return {
    answer: () =>
      new Promise<null>((resolve) => {
        release = () => resolve(null);
      }),
    release: () => release(),
  };
}

/** the users table, found by its name, so an address in a toast is not a second match */
async function usersTable(canvasElement: HTMLElement): Promise<HTMLElement> {
  return within(canvasElement).findByRole("table", { name: "Users" });
}

const rowOf = (table: HTMLElement, email: string) =>
  within(within(table).getByText(email).closest('[role="row"]') as HTMLElement);

/**
 * A dialog by its title. While a confirmation is raised over the edit sheet
 * there are two `role="dialog"` nodes, so `confirmation()` cannot tell them
 * apart.
 */
const dialogNamed = (name: string) => within(document.body).findByRole("dialog", { name });

async function press(dialog: HTMLElement, name: string): Promise<void> {
  await userEvent.click(within(dialog).getByRole("button", { name }));
}

/** cancel the confirmation and wait for it to go, leaving whatever sits under it */
async function dismiss(dialog: HTMLElement): Promise<void> {
  await press(dialog, "Cancel");
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
}

/** open the edit sheet of one account, and hand back the sheet and its queries */
async function openEditor(canvasElement: HTMLElement, email: string) {
  await userEvent.click(
    await within(canvasElement).findByRole("button", { name: `Edit ${email}` }),
  );
  const panel = await dialogNamed("Edit user");
  return { panel, form: within(panel) };
}

const SUPERADMIN_SWITCH = "Superadmin (full cross-org access)";

/**
 * Assert a confirmation paints above the sheet it was raised over. The sheet is
 * inert while the dialog is up, so nothing else in a play can tell the two
 * layers apart: a dialog stacked under the sheet's own scrim answers every
 * query and passes every other assertion.
 */
function expectPaintsOver(dialog: HTMLElement, panel: HTMLElement) {
  const layer = (node: HTMLElement) => node.closest(".fixed") as HTMLElement;
  expect(Number(getComputedStyle(layer(dialog)).zIndex)).toBeGreaterThan(
    Number(getComputedStyle(layer(panel)).zIndex),
  );
}

/** every `form_submit` outcome the UX stream holds for one confirmation */
const submits = (target: string) =>
  uxEvents()
    .filter((event) => event.action === "form_submit" && event.target === target)
    .map((event) => event.outcome);

/**
 * Deactivating goes through `ConfirmDialog`: the title names the account and
 * the body says what it costs, read off the control plane (sign-in blocked,
 * sessions ended, the keys the account minted for itself stop at the
 * gateway). A cancel sends nothing and is an abandon; a confirm is on the wire
 * with both buttons out of reach until it lands, then the dialog closes, the
 * toast says what happened and the row turns deactivated.
 */
let deactivations: Recorder;
const deactivationHold = held();
export const DeactivatingAnAccountIsConfirmedThenLands: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    deactivations = recording(accountsApi({ onPut: deactivationHold.answer }));
    return (
      <Harness fetchStub={deactivations.stub}>
        <UxScreenProvider screen="gov-users">
          <Toasted>
            <Users />
          </Toasted>
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const deactivate = "Deactivate grace@example.com";
    await userEvent.click(await canvas.findByRole("button", { name: deactivate }));
    await expect(
      await within(document.body).findByRole("heading", { name: `${deactivate}?` }),
    ).toBeInTheDocument();
    const body =
      /They can no longer sign in, every session they have open ends at once, and the virtual keys they minted for themselves stop working at the gateway\. Reactivating the account lets them sign in again and brings those keys back\./;
    await expect(within(await confirmation()).getByText(body)).toBeInTheDocument();
    // someone else's account: no line about the caller's own
    await expect(within(await confirmation()).queryByText(/signed in with/)).toBeNull();

    await cancelConfirmation();
    deactivations.expectNotSent("PUT", "/users/");
    const abandon = await expectUxEvent("form_abandon", "user-deactivate");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "user-deactivate");

    await userEvent.click(canvas.getByRole("button", { name: deactivate }));
    await confirmDestructive(body, "Deactivate");
    await expect(await deactivations.expectSentBody("PUT", "/users/user-2")).toEqual({
      deactivated: true,
    });

    // in flight: the request is on the wire, so neither button can be pressed
    const dialog = within(await confirmation());
    await waitFor(() => expect(dialog.getByRole("button", { name: "Deactivate" })).toBeDisabled());
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeDisabled();

    deactivationHold.release();
    await expectSheetClosed();
    await expectToast(canvasElement, /Deactivated grace@example\.com/);
    const table = await usersTable(canvasElement);
    await waitFor(() =>
      expect(rowOf(table, "grace@example.com").getByText("Deactivated")).toBeVisible(),
    );
    // the control now goes the other way, and says so
    await expect(
      canvas.getByRole("button", { name: "Reactivate grace@example.com" }),
    ).toBeInTheDocument();
    const submit = await expectUxEvent("form_submit", "user-deactivate");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "user-deactivate");
  },
};

/**
 * The control plane refuses the deactivation. The dialog stays open with its
 * message verbatim and a line on who may do it, and the account is untouched.
 */
export const DeactivatingAnAccountRefusedByTheServer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={accountsApi({
        onPut: () => json({ error: { message: "superadmin required" } }, 403),
      })}
    >
      <UxScreenProvider screen="gov-users">
        <Users />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Deactivate grace@example.com" }),
    );
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Deactivate" }));
    await waitFor(() => expect(dialog.getByRole("alert")).toHaveTextContent("superadmin required"));
    await waitFor(() =>
      expect(
        dialog.getByText("Editing, deactivating or deleting an account takes a superadmin."),
      ).toBeVisible(),
    );
    await waitFor(() => expect(submits("user-deactivate")).toEqual(["ok", "error"]));
    expectNoUxEvent("save_confirmed", "user-deactivate");
    // the dialog is still there to retry or cancel, and the account is unchanged
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
    await expect(
      canvas.getByRole("button", { name: "Deactivate grace@example.com" }),
    ).toBeInTheDocument();
  },
};

/**
 * Nothing in the control plane refuses a superadmin deactivating the account
 * they are signed in with, so the dialog says what it costs: they are signed
 * out now, and another superadmin has to reactivate it.
 */
export const DeactivatingYourOwnAccountSaysSo: Story = {
  render: () => (
    <Harness fetchStub={accountsApi()}>
      <SignedInAs user={ME}>
        <Users />
      </SignedInAs>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Deactivate ada@example.com" }),
    );
    await expect(
      within(await confirmation()).getByText(
        /This is the account you are signed in with: you are signed out now, and only another superadmin can reactivate it\./,
      ),
    ).toBeInTheDocument();
    await cancelConfirmation();
  },
};

/**
 * Reactivating only gives access back, so it is one click with no question, but
 * it does not look like the other direction: the icon and the label are their
 * own, and the toast says what came back.
 */
let reactivations: Recorder;
export const ReactivatingIsOneClickWithItsOwnIconAndLabel: Story = {
  render: () => {
    reactivations = recording(accountsApi());
    return (
      <Harness fetchStub={reactivations.stub}>
        <Toasted>
          <Users />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const reactivate = await canvas.findByRole("button", {
      name: "Reactivate former@example.com",
    });
    const deactivate = canvas.getByRole("button", { name: "Deactivate grace@example.com" });
    await expect(reactivate.querySelector("svg")).toHaveClass("lucide-user-check");
    await expect(deactivate.querySelector("svg")).toHaveClass("lucide-user-x");
    await expect(
      canvas.queryByRole("button", { name: "Deactivate former@example.com" }),
    ).toBeNull();

    await userEvent.click(reactivate);
    await expect(await reactivations.expectSentBody("PUT", "/users/user-3")).toEqual({
      deactivated: false,
    });
    await expectToast(canvasElement, /Reactivated former@example\.com/);
    const table = await usersTable(canvasElement);
    await waitFor(() =>
      expect(rowOf(table, "former@example.com").getByText("Active")).toBeVisible(),
    );
    await expect(within(document.body).queryByRole("dialog")).toBeNull();
  },
};

/**
 * Turning superadmin on is confirmed when the sheet is saved, since that is
 * the moment it is granted. The title names the account and the body says what
 * the flag hands over. A cancel returns to the sheet with the draft as it was
 * and sends nothing; a confirm is on the wire with both buttons out of reach
 * until it lands, then both dialogs close.
 */
let grants: Recorder;
const grantHold = held();
export const GrantingSuperadminIsConfirmedThenLands: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    grants = recording(accountsApi({ onPut: grantHold.answer }));
    return (
      <Harness fetchStub={grants.stub}>
        <UxScreenProvider screen="gov-users">
          <Toasted>
            <Users />
          </Toasted>
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const { panel, form } = await openEditor(canvasElement, "grace@example.com");
    await userEvent.click(form.getByRole("switch", { name: SUPERADMIN_SWITCH }));
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    // nothing has been sent: the save is waiting on an answer
    const ask = await dialogNamed("Make grace@example.com a superadmin?");
    expectPaintsOver(ask, panel);
    grants.expectNotSent("PUT", "/users/");
    await waitFor(() =>
      expect(
        within(ask).getByText(
          /A superadmin reaches every organization, team and project and can change every deployment-wide setting and every account, whatever roles they hold\./,
        ),
      ).toBeVisible(),
    );
    await dismiss(ask);
    grants.expectNotSent("PUT", "/users/");
    const abandon = await expectUxEvent("form_abandon", "user-superadmin-grant");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "user-superadmin-grant");
    // back in the sheet with the flag still on
    await expect(form.getByRole("switch", { name: SUPERADMIN_SWITCH })).toBeChecked();

    await userEvent.click(form.getByRole("button", { name: "Save" }));
    const again = await dialogNamed("Make grace@example.com a superadmin?");
    await press(again, "Make superadmin");
    await expect(await grants.expectSentBody("PUT", "/users/user-2")).toEqual({
      is_superadmin: true,
    });

    // in flight: the request is on the wire, so neither button can be pressed
    await waitFor(() =>
      expect(within(again).getByRole("button", { name: "Make superadmin" })).toBeDisabled(),
    );
    await expect(within(again).getByRole("button", { name: "Cancel" })).toBeDisabled();

    grantHold.release();
    await expectSheetClosed();
    await expectToast(canvasElement, /grace@example\.com updated/);
    const table = await usersTable(canvasElement);
    await waitFor(() =>
      expect(rowOf(table, "grace@example.com").getByText("Superadmin")).toBeVisible(),
    );
    const submit = await expectUxEvent("form_submit", "user-superadmin-grant");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "user-superadmin-grant");
  },
};

/**
 * The control plane refuses the grant. The confirmation stays open with the
 * message verbatim and a line on who may do it, and the sheet behind it keeps
 * the draft.
 */
export const GrantingSuperadminRefusedByTheServer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={accountsApi({
        onPut: () => json({ error: { message: "superadmin required" } }, 403),
      })}
    >
      <UxScreenProvider screen="gov-users">
        <Users />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { form } = await openEditor(canvasElement, "grace@example.com");
    await userEvent.click(form.getByRole("switch", { name: SUPERADMIN_SWITCH }));
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    const ask = await dialogNamed("Make grace@example.com a superadmin?");
    await press(ask, "Make superadmin");
    await waitFor(() =>
      expect(within(ask).getByRole("alert")).toHaveTextContent("superadmin required"),
    );
    await waitFor(() =>
      expect(
        within(ask).getByText("Editing, deactivating or deleting an account takes a superadmin."),
      ).toBeVisible(),
    );
    await waitFor(() => expect(submits("user-superadmin-grant")).toEqual(["ok", "error"]));
    expectNoUxEvent("save_confirmed", "user-superadmin-grant");

    // cancelling leaves the sheet as it was, without the refusal on it
    await dismiss(ask);
    await expect(form.getByRole("switch", { name: SUPERADMIN_SWITCH })).toBeChecked();
    await waitFor(() => expect(form.queryByRole("alert")).toBeNull());
  },
};

/**
 * Taking superadmin off someone else's account saves without a question: it
 * removes access, and the caller is not the one losing it.
 */
let demotions: Recorder;
export const TakingSuperadminOffAnotherAccountSavesWithoutAsking: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    demotions = recording(accountsApi());
    return (
      <Harness fetchStub={demotions.stub}>
        <SignedInAs user={{ ...ME, id: "user-9", email: "root@example.com" }}>
          <UxScreenProvider screen="gov-users">
            <Toasted>
              <Users />
            </Toasted>
          </UxScreenProvider>
        </SignedInAs>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const { form } = await openEditor(canvasElement, "ada@example.com");
    await userEvent.click(form.getByRole("switch", { name: SUPERADMIN_SWITCH }));
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await expect(await demotions.expectSentBody("PUT", "/users/user-1")).toEqual({
      is_superadmin: false,
    });
    await expectSheetClosed();
    await expectToast(canvasElement, /ada@example\.com updated/);
    const table = await usersTable(canvasElement);
    await waitFor(() =>
      expect(rowOf(table, "ada@example.com").queryByText("Superadmin")).toBeNull(),
    );
    expectNoUxEvent("form_submit", "user-superadmin-remove");
    expectNoUxEvent("form_submit", "user-superadmin-grant");
  },
};

/** A save that does not touch the flag goes straight out: there is nothing to confirm. */
let renames: Recorder;
export const SavingAnEmailAsksNothing: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    renames = recording(accountsApi());
    return (
      <Harness fetchStub={renames.stub}>
        <UxScreenProvider screen="gov-users">
          <Users />
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const { form } = await openEditor(canvasElement, "grace@example.com");
    const email = form.getByLabelText("Email");
    await userEvent.clear(email);
    await userEvent.type(email, "grace.hopper@example.com");
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await expect(await renames.expectSentBody("PUT", "/users/user-2")).toEqual({
      email: "grace.hopper@example.com",
    });
    await expectSheetClosed();
    expectNoUxEvent("form_submit", "user-superadmin-grant");
    expectNoUxEvent("form_submit", "user-superadmin-remove");
  },
};

/**
 * Nothing in the control plane refuses a superadmin dropping their own flag,
 * and the next request is then made without it, so this one save asks first
 * and says what it costs. A cancel sends nothing; a confirm lands and the
 * badge leaves the row.
 */
let selfDemotion: Recorder;
export const RemovingYourOwnSuperadminIsConfirmed: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    selfDemotion = recording(accountsApi());
    return (
      <Harness fetchStub={selfDemotion.stub}>
        <SignedInAs user={ME}>
          <UxScreenProvider screen="gov-users">
            <Toasted>
              <Users />
            </Toasted>
          </UxScreenProvider>
        </SignedInAs>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const { form } = await openEditor(canvasElement, "ada@example.com");
    await userEvent.click(form.getByRole("switch", { name: SUPERADMIN_SWITCH }));
    await userEvent.click(form.getByRole("button", { name: "Save" }));

    const ask = await dialogNamed("Remove superadmin from ada@example.com?");
    await waitFor(() =>
      expect(
        within(ask).getByText(
          /This is your own account\. Without superadmin you lose every deployment-wide setting and every organization where you hold no role, and only another superadmin can give it back\./,
        ),
      ).toBeVisible(),
    );
    await dismiss(ask);
    selfDemotion.expectNotSent("PUT", "/users/");
    const abandon = await expectUxEvent("form_abandon", "user-superadmin-remove");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "user-superadmin-remove");

    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await press(await dialogNamed("Remove superadmin from ada@example.com?"), "Remove superadmin");
    await expect(await selfDemotion.expectSentBody("PUT", "/users/user-1")).toEqual({
      is_superadmin: false,
    });
    await expectSheetClosed();
    const table = await usersTable(canvasElement);
    await waitFor(() =>
      expect(rowOf(table, "ada@example.com").queryByText("Superadmin")).toBeNull(),
    );
    const submit = await expectUxEvent("form_submit", "user-superadmin-remove");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "user-superadmin-remove");
  },
};

/**
 * Delete is raised from the edit sheet through `ConfirmDialog`, which stays
 * open behind it. The body says the account leaves every organization, what
 * goes with it, and points at deactivating instead. A cancel sends nothing
 * and returns to the sheet; a confirm is on the wire with both buttons out of
 * reach until it lands, then the dialog and the sheet close and the row goes.
 */
let deletions: Recorder;
const deletionHold = held();
export const DeletingAnAccountIsConfirmedThenLands: Story = {
  beforeEach: recordUxEvents,
  render: () => {
    deletions = recording(accountsApi({ onDelete: deletionHold.answer }));
    return (
      <Harness fetchStub={deletions.stub}>
        <UxScreenProvider screen="gov-users">
          <Toasted>
            <Users />
          </Toasted>
        </UxScreenProvider>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const { panel, form } = await openEditor(canvasElement, "grace@example.com");
    await userEvent.click(form.getByRole("button", { name: "Delete" }));

    const ask = await dialogNamed("Delete grace@example.com?");
    expectPaintsOver(ask, panel);
    const body =
      /The account leaves every organization, not only this one, with all its roles and sessions, and the virtual keys it minted for itself are disabled\. This cannot be undone\. To block sign-in and keep the account, deactivate it instead\./;
    await expect(within(ask).getByText(body)).toBeInTheDocument();
    await expect(within(ask).queryByText(/signed in with/)).toBeNull();

    await dismiss(ask);
    deletions.expectNotSent("DELETE", "/users/");
    const abandon = await expectUxEvent("form_abandon", "user-delete");
    await expect(abandon.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", "user-delete");
    // the sheet is still there, and Delete no longer expands a panel of its own
    await expect(panel).toBeInTheDocument();

    await userEvent.click(form.getByRole("button", { name: "Delete" }));
    const again = await dialogNamed("Delete grace@example.com?");
    await press(again, "Delete account");
    await deletions.expectSent("DELETE", "/users/user-2");

    // in flight: the request is on the wire, so neither button can be pressed
    await waitFor(() =>
      expect(within(again).getByRole("button", { name: "Delete account" })).toBeDisabled(),
    );
    await expect(within(again).getByRole("button", { name: "Cancel" })).toBeDisabled();

    deletionHold.release();
    await expectSheetClosed();
    await expectToast(canvasElement, /grace@example\.com deleted/);
    const table = await usersTable(canvasElement);
    await waitFor(() => expect(within(table).queryByText("grace@example.com")).toBeNull());
    await expect(within(table).getByText("ada@example.com")).toBeVisible();
    const submit = await expectUxEvent("form_submit", "user-delete");
    await expect(submit.outcome).toBe("ok");
    await expectUxEvent("save_confirmed", "user-delete");
  },
};

/**
 * The control plane refuses the delete. The dialog stays open with its message
 * verbatim and a line on who may do it; the sheet and the account are still
 * there.
 */
export const DeletingAnAccountRefusedByTheServer: Story = {
  beforeEach: recordUxEvents,
  render: () => (
    <Harness
      fetchStub={accountsApi({
        onDelete: () => json({ error: { message: "superadmin required" } }, 403),
      })}
    >
      <UxScreenProvider screen="gov-users">
        <Users />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { panel, form } = await openEditor(canvasElement, "grace@example.com");
    await userEvent.click(form.getByRole("button", { name: "Delete" }));
    const ask = await dialogNamed("Delete grace@example.com?");
    await press(ask, "Delete account");
    await waitFor(() =>
      expect(within(ask).getByRole("alert")).toHaveTextContent("superadmin required"),
    );
    await waitFor(() =>
      expect(
        within(ask).getByText("Editing, deactivating or deleting an account takes a superadmin."),
      ).toBeVisible(),
    );
    await waitFor(() => expect(submits("user-delete")).toEqual(["ok", "error"]));
    expectNoUxEvent("save_confirmed", "user-delete");
    await expect(panel).toBeInTheDocument();
  },
};

/**
 * The account was deleted elsewhere while the dialog was open. The control
 * plane answers 404, so the dialog says so and the list is read again and
 * drops the row.
 */
export const DeletingAnAccountAlreadyGoneSaysSo: Story = {
  render: () => (
    <Harness
      fetchStub={accountsApi({
        onDelete: (id, state) => {
          state.accounts = state.accounts.filter((user) => user.id !== id);
          return json({ error: { message: `user ${id} not found` } }, 404);
        },
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { form } = await openEditor(canvasElement, "grace@example.com");
    await userEvent.click(form.getByRole("button", { name: "Delete" }));
    const ask = await dialogNamed("Delete grace@example.com?");
    await press(ask, "Delete account");
    await waitFor(() =>
      expect(
        within(ask).getByText(
          "This account no longer exists, most likely because it was just deleted. The list has been refreshed.",
        ),
      ).toBeVisible(),
    );
    await expect(within(ask).getByRole("alert")).toHaveTextContent("user user-2 not found");
    const table = await usersTable(canvasElement);
    await waitFor(() => expect(within(table).queryByText("grace@example.com")).toBeNull());
  },
};

/**
 * Nothing in the control plane refuses a superadmin deleting the account they
 * are signed in with, so the dialog says what it costs.
 */
export const DeletingYourOwnAccountSaysSo: Story = {
  render: () => (
    <Harness fetchStub={accountsApi()}>
      <SignedInAs user={ME}>
        <Users />
      </SignedInAs>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { form } = await openEditor(canvasElement, "ada@example.com");
    await userEvent.click(form.getByRole("button", { name: "Delete" }));
    const ask = await dialogNamed("Delete ada@example.com?");
    await expect(
      within(ask).getByText(
        /This is the account you are signed in with: you are signed out now and cannot sign in again\./,
      ),
    ).toBeInTheDocument();
    await dismiss(ask);
  },
};

/** A superadmin may use every account control, asserted through the gate's own answer. */
export const AccountControlsOpenToASuperadmin: Story = {
  render: () => (
    <Harness fetchStub={accountsApi()} role="superadmin">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Edit grace@example.com");
    await expectAllowed(canvasElement, "Deactivate grace@example.com");
    await expectAllowed(canvasElement, "Reactivate former@example.com");
  },
};

const LAST_SUPERADMIN_REFUSAL = () =>
  json(
    {
      error: {
        code: "last_superadmin",
        message: "this is the last active superadmin; make another account superadmin first",
      },
    },
    409,
  );
const LAST_SUPERADMIN_HINT =
  "This is the last active superadmin, so the change was refused. Make another account superadmin first, then try again.";

/**
 * The control plane refuses a deactivate that would leave no active superadmin
 * with a 409 `last_superadmin` (#2471). The list is org-scoped, so the screen
 * cannot warn before the click; the dialog stays open and says what to do.
 */
export const DeactivatingTheLastSuperadminSaysWhatToDo: Story = {
  render: () => (
    <Harness fetchStub={accountsApi({ onPut: LAST_SUPERADMIN_REFUSAL })}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole("button", { name: "Deactivate ada@example.com" }),
    );
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Deactivate" }));
    await waitFor(() => expect(dialog.getByText(LAST_SUPERADMIN_HINT)).toBeVisible());
    await expect(dialog.getByRole("alert")).toHaveTextContent("last active superadmin");
    await expect(within(document.body).getByRole("dialog")).toBeInTheDocument();
  },
};

/** The same refusal on a delete raised from the edit sheet. */
export const DeletingTheLastSuperadminSaysWhatToDo: Story = {
  render: () => (
    <Harness fetchStub={accountsApi({ onDelete: LAST_SUPERADMIN_REFUSAL })}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { panel, form } = await openEditor(canvasElement, "ada@example.com");
    await userEvent.click(form.getByRole("button", { name: "Delete" }));
    const ask = await dialogNamed("Delete ada@example.com?");
    await press(ask, "Delete account");
    await waitFor(() => expect(within(ask).getByText(LAST_SUPERADMIN_HINT)).toBeVisible());
    await expect(panel).toBeInTheDocument();
  },
};

/** A demote saved straight from the sheet carries the hint beside the server's message. */
export const DemotingTheLastSuperadminSaysWhatToDo: Story = {
  render: () => (
    <Harness fetchStub={accountsApi({ onPut: LAST_SUPERADMIN_REFUSAL })}>
      <SignedInAs user={{ ...ME, id: "user-9", email: "root@example.com" }}>
        <Users />
      </SignedInAs>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const { form } = await openEditor(canvasElement, "ada@example.com");
    await userEvent.click(form.getByRole("switch", { name: SUPERADMIN_SWITCH }));
    await userEvent.click(form.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(form.getByText(/Make another account superadmin first/)).toBeVisible(),
    );
    await expect(form.getByRole("button", { name: "Save" })).toBeInTheDocument();
  },
};

/**
 * Inviting an address that already has a live invitation replaces it, so the
 * sheet says the old link stops working before the operator sends (#2471).
 */
export const InvitingAPendingAddressSaysItReplacesTheOldLink: Story = {
  render: () => (
    <Harness fetchStub={invitationsApi()}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await clickWhenEnabled(canvasElement, /invite user/i);
    const form = within(sheet());
    const email = await form.findByLabelText("Email");
    await userEvent.type(email, "other@example.com");
    await expect(form.queryByText(/already has a pending invitation/)).toBeNull();
    await userEvent.clear(email);
    await userEvent.type(email, "Newcomer@Example.com");
    await waitFor(() =>
      expect(
        form.getByText(
          "Newcomer@Example.com already has a pending invitation. Sending another replaces it, and the old link stops working.",
        ),
      ).toBeVisible(),
    );
  },
};

/**
 * The invite sheet explains its two ways in, and the explanation is a sentence
 * the panel has to hold whole: at a 1024px window it was cut at "…or create the
 * account with a password y…" (#2812). It wraps, in both locales.
 */
function inviteSubtitleIsWhole(locale: "en" | "ru") {
  const cat = locale === "ru" ? ru : en;
  return {
    ...atSplit,
    globals: { ...atSplit.globals, locale },
    render: () => (
      <Harness fetchStub={loaded}>
        <Users />
      </Harness>
    ),
    play: async ({ canvasElement }: { canvasElement: HTMLElement }) => {
      await clickWhenEnabled(canvasElement, new RegExp(cat.pages.users.inviteAction, "i"));
      const subtitle = await within(sheet()).findByText(cat.pages.users.inviteSubtitle);
      await expectNotTruncated(subtitle);
      await expectInViewport(subtitle);
    },
  };
}

export const InviteSubtitleIsWholeAt1024: Story = inviteSubtitleIsWhole("en");
export const InviteSubtitleIsWholeAt1024InRussian: Story = inviteSubtitleIsWhole("ru");

// -------------------------- accounts with no role anywhere (#2804)

/**
 * The first administrator `rolter-seed --admin-email` creates: a superadmin
 * who holds no membership, so no org's people list names them.
 */
const SEEDED_ADMIN: UserRow = {
  id: "user-0",
  email: "root@example.com",
  is_superadmin: true,
  deactivated_at: null,
  created_at: "2026-01-01T00:00:00Z",
};

/**
 * What `GET /orgs/{org}/users` answers: the org's members, plus the accounts
 * with no membership anywhere when `include_unassigned=true` is asked for. The
 * control plane only honours the flag for a superadmin, so a story that signs
 * in as anyone else sends none.
 */
function orgWithUnassigned(): Recorder {
  return recording(
    scoped(async (input) => {
      const url = String(input);
      if (url.includes("/invitations")) return json([]);
      if (url.includes("/memberships")) return json(MEMBERSHIPS);
      if (url.includes("/users")) {
        return json(url.includes("include_unassigned=true") ? [SEEDED_ADMIN, ...USERS] : USERS);
      }
      return json([]);
    }),
  );
}

const unassignedAsSuperadmin = orgWithUnassigned();

/**
 * A superadmin asks for the accounts that hold no role, so their own row is on
 * the screen and can be edited. The row says it has no role in this org
 * instead of looking like a load that failed, and nothing else changes.
 */
export const SuperadminSeesAnAccountWithNoRole: Story = {
  render: () => (
    <Harness fetchStub={unassignedAsSuperadmin.stub} role="superadmin">
      <SignedInAs user={SEEDED_ADMIN}>
        <Users />
      </SignedInAs>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const table = await usersTable(canvasElement);
    const root = await within(table).findByText("root@example.com");
    const row = within(root.closest('[role="row"]') as HTMLElement);
    await expect(row.getByText("Superadmin")).toBeVisible();
    await waitFor(() => expect(row.getByText("no role in this org")).toBeVisible());
    // an account that does hold a role keeps showing it
    await expect(rowOf(table, "grace@example.com").getByText("Member")).toBeVisible();
    await expect(rowOf(table, "grace@example.com").queryByText("no role in this org")).toBeNull();

    // the flag is in the request, which is how the control plane knows to add them
    await waitFor(() =>
      expect(
        unassignedAsSuperadmin.calls.some(
          (c) => c.method === "GET" && c.url.endsWith("/orgs/org-1/users?include_unassigned=true"),
        ),
      ).toBe(true),
    );

    // and the caller's own row is theirs to edit
    await expectAllowed(canvasElement, "Edit root@example.com");
    await userEvent.click(within(table).getByRole("button", { name: "Edit root@example.com" }));
    await expect(await dialogNamed("Edit user")).toBeVisible();
  },
};

/** The same row in Russian, where the sentence is longer than the English one. */
export const AccountWithNoRoleInRussian: Story = {
  globals: { locale: "ru" },
  render: () => (
    <Harness fetchStub={orgWithUnassigned().stub} role="superadmin">
      <SignedInAs user={SEEDED_ADMIN}>
        <Users />
      </SignedInAs>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const table = await within(canvasElement).findByRole("table", { name: "Пользователи" });
    const root = await within(table).findByText("root@example.com");
    const row = within(root.closest('[role="row"]') as HTMLElement);
    await waitFor(() => expect(row.getByText("нет роли в этой организации")).toBeVisible());
  },
};

const unassignedAsAdmin = orgWithUnassigned();

/**
 * Anyone who is not a superadmin never asks for them: the flag would change
 * nothing server-side, and a request that does not carry it cannot be mistaken
 * for one that does.
 */
export const OrgAdminDoesNotAskForAccountsWithNoRole: Story = {
  render: () => (
    <Harness fetchStub={unassignedAsAdmin.stub} role="admin">
      <SignedInAs user={{ ...USERS[1], is_superadmin: false }}>
        <Users />
      </SignedInAs>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const table = await usersTable(canvasElement);
    await expect(await within(table).findByText("grace@example.com")).toBeVisible();
    await expect(within(table).queryByText("root@example.com")).toBeNull();
    await expect(
      unassignedAsAdmin.calls.filter((c) => c.url.includes("include_unassigned")),
    ).toHaveLength(0);
    await expect(
      unassignedAsAdmin.calls.some(
        (c) => c.method === "GET" && c.url.endsWith("/orgs/org-1/users"),
      ),
    ).toBe(true);
  },
};

// ---------------------------------------------------------------------------
// #3010: the access profiles a person holds, and handing one out from here
//
// The Access Profiles cards answer "who does this profile reach"; this column
// answers the other half, "what can this person do and why", through the same
// assignment endpoints.

const SUPPORT: AccessProfileRow = {
  id: "p-1",
  org_id: "org-1",
  slug: "support",
  name: "Support engineers",
  description: "Read-only on production routes",
  created_at: "",
  updated_at: "",
};
const ONCALL: AccessProfileRow = {
  ...SUPPORT,
  id: "p-2",
  slug: "on-call",
  name: "On-call",
  description: null,
};
const profileDetail = (
  profile: AccessProfileRow,
  assignments: { id: string; user_id: string | null }[],
) => ({
  ...profile,
  roles: [],
  policy: null,
  assignments: assignments.map((a) => ({
    ...a,
    profile_id: profile.id,
    team_id: null,
    created_at: "",
  })),
});

/** the profile reads over the users fixture, holding what posts and deletes changed */
function profileStub(initial: Record<string, string[]> = { "p-1": ["user-2"], "p-2": [] }) {
  const held = new Map(Object.entries(initial).map(([id, users]) => [id, [...users]]));
  return recording(async (input, init) => {
    const url = String(input);
    const post = /\/access-profiles\/([^/]+)\/assignments$/.exec(url);
    if (init?.method === "POST" && post) {
      const body = JSON.parse(String(init.body)) as { user_id: string };
      held.set(post[1], [...(held.get(post[1]) ?? []), body.user_id]);
      return json({ id: "a-new", profile_id: post[1], created_at: "", ...body, team_id: null });
    }
    const del = /\/access-profile-assignments\/a-(p-\d)-(user-\d)$/.exec(url);
    if (init?.method === "DELETE" && del) {
      held.set(
        del[1],
        (held.get(del[1]) ?? []).filter((u) => u !== del[2]),
      );
      return json({}, 204);
    }
    if (/\/access-profile-assignments(\?|$)/.test(url)) {
      return json(
        [...held].flatMap(([profileId, users]) =>
          users.map((user_id) => ({
            id: `a-${profileId}-${user_id}`,
            profile_id: profileId,
            user_id,
            team_id: null,
            created_at: "",
          })),
        ),
      );
    }
    const detail = /\/access-profiles\/(p-\d)$/.exec(url);
    if (detail) {
      const profile = detail[1] === "p-1" ? SUPPORT : ONCALL;
      return json(
        profileDetail(
          profile,
          (held.get(detail[1]) ?? []).map((user_id) => ({
            id: `a-${detail[1]}-${user_id}`,
            user_id,
          })),
        ),
      );
    }
    if (/\/access-profiles$/.test(url)) return json([SUPPORT, ONCALL]);
    if (url.includes("/memberships")) return json(MEMBERSHIPS);
    if (url.includes("/users")) return json(USERS);
    return json([]);
  });
}
const withProfiles = (rec: Recorder): FetchStub => scoped(rec.stub);

const userRow = (canvas: ReturnType<typeof within>, email: string) =>
  within(canvas.getByText(email).closest('[role="row"]') as HTMLElement);

const listsProfiles = profileStub();
export const ListsTheProfilesAPersonHolds: Story = {
  render: () => {
    const rec = listsProfiles;
    return (
      <Harness fetchStub={withProfiles(rec)} role="admin">
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("grace@example.com");
    const grace = userRow(canvas, "grace@example.com");
    const list = await grace.findByRole("list", {
      name: "Access profiles held by grace@example.com",
    });
    await expect(within(list).getByText("Support engineers")).toBeVisible();
    // the assignments come in one org-wide read, not a detail per profile (#3025)
    await listsProfiles.expectSent("GET", "/access-profile-assignments");
    listsProfiles.expectNotSent("GET", "/access-profiles/p-1");
    // a person who holds none reads as such, never as a blank cell
    await waitFor(() =>
      expect(userRow(canvas, "ada@example.com").getByText("no access profile")).toBeVisible(),
    );
  },
};

export const ProfilesFailedToLoad: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={scoped(async (input) =>
        String(input).includes("/access-profile")
          ? json({ error: { message: "boom" } }, 500)
          : String(input).includes("/memberships")
            ? json(MEMBERSHIPS)
            : json(USERS),
      )}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("grace@example.com");
    // an unread list is not "holds nothing"
    await waitFor(() =>
      expect(userRow(canvas, "grace@example.com").getByText("profiles not loaded")).toBeVisible(),
    );
    await expect(canvas.queryByText("no access profile")).toBeNull();
  },
};

export const ProfilesStillLoading: Story = {
  render: () => (
    <Harness
      role="admin"
      fetchStub={scoped((input) =>
        String(input).includes("/access-profile")
          ? new Promise<Response>(() => {})
          : Promise.resolve(
              String(input).includes("/memberships") ? json(MEMBERSHIPS) : json(USERS),
            ),
      )}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("grace@example.com");
    await expect(canvas.queryByText("no access profile")).toBeNull();
    await expect(canvas.queryByText("profiles not loaded")).toBeNull();
  },
};

const assignsFromUsers = profileStub();
export const AssignsAProfileToAPerson: Story = {
  render: () => (
    <Harness fetchStub={withProfiles(assignsFromUsers)} role="admin">
      <Toasted>
        <Users />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("grace@example.com");
    await expectAllowed(canvasElement, "Assign an access profile to grace@example.com");
    await userEvent.click(
      canvas.getByRole("button", { name: "Assign an access profile to grace@example.com" }),
    );

    const form = within(sheet());
    const listbox = await openOptions(
      await form.findByRole("combobox", { name: "Access profile" }),
    );
    // what she holds already is not offered twice
    await waitFor(() =>
      expect(within(listbox).getByRole("option", { name: /On-call/ })).toBeVisible(),
    );
    await expect(within(listbox).queryByRole("option", { name: /Support engineers/ })).toBeNull();
    await userEvent.click(within(listbox).getByRole("option", { name: /On-call/ }));
    await userEvent.click(form.getByRole("button", { name: "Assign profile" }));

    const body = await assignsFromUsers.expectSentBody("POST", "/access-profiles/p-2/assignments");
    expect(body).toEqual({ user_id: "user-2" });
    await expectSheetClosed();
    await expectToast(canvasElement, /On-call assigned to grace@example.com/);
    // the outcome is on her row, read back from the profile
    const list = await userRow(canvas, "grace@example.com").findByRole("list", {
      name: "Access profiles held by grace@example.com",
    });
    await waitFor(() => expect(within(list).getByText("On-call")).toBeVisible());
  },
};

const removesFromUsers = profileStub();
export const RemovesAProfileFromAPerson: Story = {
  render: () => (
    <Harness fetchStub={withProfiles(removesFromUsers)} role="admin">
      <Toasted>
        <Users />
      </Toasted>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("grace@example.com");
    await expectAllowed(canvasElement, "Remove Support engineers from grace@example.com");
    await userEvent.click(
      canvas.getByRole("button", { name: "Remove Support engineers from grace@example.com" }),
    );
    await confirmDestructive(
      /Remove grace@example.com from Support engineers/,
      "Remove assignment",
    );

    await removesFromUsers.expectSent("DELETE", "/access-profile-assignments/a-p-1-user-2");
    await expectSheetClosed();
    await waitFor(() =>
      expect(userRow(canvas, "grace@example.com").getByText("no access profile")).toBeVisible(),
    );
  },
};

// both controls are `access_profile_assignment` writes at the org, which a
// viewer reads and may not change
export const ProfileAssignmentRefusedToAViewer: Story = {
  render: () => (
    <Harness fetchStub={withProfiles(profileStub())} role="viewer">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByText("grace@example.com");
    await expectRefused(canvasElement, "Assign an access profile to grace@example.com");
    await expectRefused(canvasElement, "Remove Support engineers from grace@example.com");
  },
};

/**
 * The sender is named by the row itself, so a caller whose users list is
 * refused (a team admin reads only the people in their own teams) still sees
 * who sent each invitation (#2325).
 */
export const PendingInvitationsNameTheirSenderWithoutTheUsersList: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/invitations")) return json(INVITATIONS);
        if (url.includes("/memberships")) return json(MEMBERSHIPS);
        if (url.includes("/users")) return json({ error: { message: "forbidden" } }, 403);
        return json([]);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const section = within(await pendingSection(canvasElement));
    const row = (email: string) =>
      within(section.getByText(email).closest('[role="row"]') as HTMLElement);
    await expect(await section.findByText("lead@example.com")).toBeVisible();
    await expect(row("lead@example.com").getByText("grace@example.com")).toBeVisible();
    await expect(row("stale@example.com").getByText("dana@elsewhere.example")).toBeVisible();
    await expect(row("everyone@example.com").getByText("not recorded")).toBeVisible();
  },
};

// ------------------------------------------ an administrator resets a password (#2240)

const RESET_URL = "https://rolter.local/reset-password/rolter_reset_one-time";
const RESET_GRACE = "Reset the password of grace@example.com";

/** the control plane's answer to an issued reset */
const issuedReset = (sessions: number) => () =>
  json({
    reset: { id: "reset-1", user_id: "user-2", expires_at: inDays(1) },
    token: "rolter_reset_one-time",
    reset_url: RESET_URL,
    sessions_revoked: sessions,
  });

/**
 * A reset asks first, saying what it does: a one-time link and never a
 * password, the open sessions end now, the old password and any second factor
 * stay. A cancel sends nothing.
 */
let resetCancelled: Recorder;
export const ResetPasswordAsksBeforeIssuingAndCancelSendsNothing: Story = {
  render: () => {
    resetCancelled = recording(directory());
    return (
      <Harness fetchStub={resetCancelled.stub}>
        <Users />
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: RESET_GRACE }));
    const dialog = within(await confirmation());
    await waitFor(() =>
      expect(
        dialog.getByRole("heading", { name: "Reset the password of grace@example.com?" }),
      ).toBeVisible(),
    );
    await waitFor(() => expect(dialog.getByText(/one-time link, never a password/)).toBeVisible());
    await waitFor(() => expect(dialog.getByText(/any second factor stays in place/)).toBeVisible());
    await cancelConfirmation();
    await expect(resetCancelled.calls.filter((c) => c.method === "POST")).toHaveLength(0);
  },
};

/**
 * Confirming issues the link: the request names the org and the member, the
 * link is shown once in the shared reveal dialog with how many sessions ended,
 * and closing it asks first because nobody has copied it.
 */
let resetIssued: Recorder;
export const ResetPasswordIssuesAOneTimeLink: Story = {
  render: () => {
    resetIssued = recording(
      scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST" && url.includes("/password-reset")) return issuedReset(2)();
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      }),
    );
    return (
      <Harness fetchStub={resetIssued.stub}>
        <Toasted>
          <Users />
        </Toasted>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: RESET_GRACE }));
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Issue reset link" }));
    await resetIssued.expectSent("POST", "/orgs/org-1/users/user-2/password-reset");

    const reveal = within(await confirmation());
    await waitFor(() => expect(reveal.getByText(RESET_URL)).toBeVisible());
    await expect(reveal.getByRole("heading", { name: "Password reset link" })).toBeVisible();
    await expect(reveal.getByText("2 open sessions of theirs were signed out.")).toBeVisible();
    await expectToast(canvasElement, /Reset link issued for grace@example\.com/);

    // not copied: closing asks first, and keeping it open leaves the link
    await userEvent.click(reveal.getByRole("button", { name: "Done" }));
    await answerSecretClosePrompt(false);
    await expect(reveal.getByText(RESET_URL)).toBeVisible();
    await userEvent.click(reveal.getByRole("button", { name: "Done" }));
    await answerSecretClosePrompt(true);
    await expectSheetClosed();
  },
};

/**
 * The control plane refuses (an account that signs in through single sign-on
 * has no password to reset): the dialog stays open with its message and no link
 * is shown.
 */
export const ResetPasswordRefusedKeepsTheDialogOpen: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input, init) => {
        const url = String(input);
        if (init?.method === "POST" && url.includes("/password-reset")) {
          return json(
            {
              error: {
                message: "the account has no local password; it signs in through single sign-on",
              },
            },
            400,
          );
        }
        if (url.includes("/invitations")) return json([]);
        return url.includes("/memberships") ? json(MEMBERSHIPS) : json(USERS);
      })}
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: RESET_GRACE }));
    const dialog = within(await confirmation());
    await userEvent.click(dialog.getByRole("button", { name: "Issue reset link" }));
    await waitFor(() => expect(dialog.getByRole("alert")).toHaveTextContent(/no local password/));
    await expect(within(document.body).queryByText(RESET_URL)).toBeNull();
  },
};

/** A deactivated account has nothing to reset: the row offers no reset action. */
export const ResetPasswordIsNotOfferedForADeactivatedAccount: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("button", { name: RESET_GRACE })).toBeVisible();
    await expect(
      canvas.queryByRole("button", { name: "Reset the password of former@example.com" }),
    ).toBeNull();
  },
};

// ---------------------------------------------------------------------------
// #3127: narrow the list to a team or a project, from the screen or a link

/** the picker above the table, which keeps its own name rather than the placeholder */
const scopeFilter = (canvas: ReturnType<typeof within>) =>
  canvas.getByRole("combobox", { name: "Filter by team or project" });

/** a team's people include the people of its projects; an org-wide grant is no team's */
export const FiltersToATeamFromTheScreen: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("ada@example.com")).toBeInTheDocument();
    await waitFor(() => expect(scopeFilter(canvas)).toBeEnabled());
    await pickOption(scopeFilter(canvas), "Platform");
    // grace holds a role on the team, former on one of its projects
    await waitFor(() => expect(canvas.queryByText("ada@example.com")).toBeNull());
    await expect(canvas.getByText("grace@example.com")).toBeVisible();
    await expect(canvas.getByText("former@example.com")).toBeVisible();
  },
};

export const FiltersToAProject: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("ada@example.com")).toBeInTheDocument();
    await waitFor(() => expect(scopeFilter(canvas)).toBeEnabled());
    await pickOption(scopeFilter(canvas), /^Gateway/);
    await waitFor(() => expect(canvas.queryByText("grace@example.com")).toBeNull());
    await expect(canvas.getByText("former@example.com")).toBeVisible();
  },
};

/** a link from the team sheet lands on the team's people, with the picker showing it */
export const OpensOnTheTeamAnAddressNames: Story = {
  render: () => (
    <Harness fetchStub={loaded} route="/gov-users?team=team-1">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("grace@example.com")).toBeVisible();
    await expect(canvas.queryByText("ada@example.com")).toBeNull();
    await waitFor(() => expect(scopeFilter(canvas)).toHaveValue("Platform"));
  },
};

/** nobody holds a role in the team: the empty state names the filter and clears it */
export const ATeamWithNoPeopleShowsTheNoMatchState: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/memberships", () => []],
        ["/users", () => USERS],
      ])}
      route="/gov-users?team=team-1"
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("No users match")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Clear filters" }));
    await expect(await canvas.findByText("ada@example.com")).toBeVisible();
  },
};

/** an id the organization does not have says so rather than listing nobody without a reason */
export const AnUnknownTeamIdIsNamed: Story = {
  render: () => (
    <Harness fetchStub={loaded} route="/gov-users?team=no-such-team">
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("No such team or project")).toBeVisible();
    await expect(canvas.getByText(/no-such-team/)).toBeVisible();
    await expect(canvas.queryByText("ada@example.com")).toBeNull();
    await userEvent.click(canvas.getByRole("button", { name: "Clear filters" }));
    await expect(await canvas.findByText("ada@example.com")).toBeVisible();
  },
};

/** the list does not show everyone as a match while the memberships the filter needs are in flight */
export const TheFilterHoldsBackWhileMembershipsLoad: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async (input) => {
        const url = String(input);
        if (url.includes("/memberships")) return new Promise<Response>(() => {});
        return json(url.includes("/users") ? USERS : []);
      })}
      route="/gov-users?team=team-1"
    >
      <Users />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectSkeleton(canvasElement);
    await expect(canvas.queryByText("ada@example.com")).toBeNull();
    await expect(canvas.queryByText("No users match")).toBeNull();
  },
};
