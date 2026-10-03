import * as React from "react";

import {
  ApiError,
  fetchMe,
  isOpenModeNoSession,
  setSessionExpiredHandler,
  type MeMembership,
  type UserRow,
} from "@/lib/api";

// client session state. a real login (POST /api/v1/auth/login) stores an opaque
// bearer token that api.ts attaches to every request, which is what the
// self-service /me/* endpoints need. login stays optional: against an open-mode
// deployment (no accounts) the Login screen falls back to an email-only gate
// with no token, and the admin dashboard keeps working exactly as before.
//
// A stored token is revalidated against GET /api/v1/auth/me on boot (#1196):
// nothing used to re-check it, so a session that expired while the tab was
// closed came back looking signed in and every screen failed on its own with
// the dead token still attached.

/**
 * `checking` while the stored token is being revalidated on boot; `ready`
 * once the answer is in (or once it is clear there is nothing to check).
 */
export type AuthStatus = "checking" | "ready";

/**
 * What the session knows about the account. `/auth/me` sends a full
 * [`UserRow`]; the login response still sends a subset of it (#1178), so the
 * shared shape is the three fields both carry plus whatever else arrived.
 */
export type SessionUser = Pick<UserRow, "id" | "email" | "is_superadmin"> & Partial<UserRow>;

interface AuthState {
  email: string | null;
  token: string | null;
  /**
   * The account behind the session: from `/auth/me` when the control plane
   * answered, otherwise the blob cached at login. `null` in an email-only
   * open-mode session, which has no account at all.
   */
  user: SessionUser | null;
  /** the account's role grants, from `/auth/me`; empty until it answers */
  memberships: MeMembership[];
  status: AuthStatus;
  /** the previous session was rejected — the login screen says so */
  expired: boolean;
  signIn: (email: string, token?: string | null, user?: SessionUser | null) => void;
  signOut: () => void;
}

const EMAIL_KEY = "rolter.session.email";
const TOKEN_KEY = "rolter.session.token";
// the login response's user blob, kept so a reload that cannot reach
// /auth/me still knows whether this account is a superadmin
const USER_KEY = "rolter.session.user";
const AuthContext = React.createContext<AuthState | null>(null);

function readStoredUser(): SessionUser | null {
  try {
    const raw = localStorage.getItem(USER_KEY);
    return raw ? (JSON.parse(raw) as SessionUser) : null;
  } catch {
    // unreadable or not json — treated as "not cached", never as a failure
    return null;
  }
}

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [email, setEmail] = React.useState<string | null>(() => localStorage.getItem(EMAIL_KEY));
  const [token, setToken] = React.useState<string | null>(() => localStorage.getItem(TOKEN_KEY));
  const [user, setUser] = React.useState<SessionUser | null>(readStoredUser);
  const [memberships, setMemberships] = React.useState<MeMembership[]>([]);
  // only a stored token is worth checking; an email-only session has nothing
  // to revalidate, so it must not sit behind a placeholder
  const [status, setStatus] = React.useState<AuthStatus>(() =>
    localStorage.getItem(TOKEN_KEY) ? "checking" : "ready",
  );
  const [expired, setExpired] = React.useState(false);

  const clearSession = React.useCallback(() => {
    localStorage.removeItem(EMAIL_KEY);
    localStorage.removeItem(TOKEN_KEY);
    localStorage.removeItem(USER_KEY);
    setEmail(null);
    setToken(null);
    setUser(null);
    setMemberships([]);
  }, []);

  // any request that carried the token and came back 401 means the same thing
  // as a rejected /auth/me: sign out once, here, with the notice on the login
  // screen — instead of every screen rendering its own dead end
  React.useEffect(
    () =>
      setSessionExpiredHandler(() => {
        clearSession();
        setExpired(true);
        setStatus("ready");
      }),
    [clearSession],
  );

  React.useEffect(() => {
    if (!token) return;
    let live = true;
    void fetchMe()
      .then((me) => {
        if (!live) return;
        localStorage.setItem(USER_KEY, JSON.stringify(me.user));
        localStorage.setItem(EMAIL_KEY, me.user.email);
        setUser(me.user);
        setMemberships(me.memberships);
        setEmail(me.user.email);
      })
      .catch((err) => {
        if (!live) return;
        // a 401 is the server saying this token is no good. anything else —
        // a network failure, a 5xx, or the 404 of a control plane that does
        // not mount /auth/* at all — is the control plane blinking, and
        // locking the operator out over it would be the worse bug
        if (err instanceof ApiError && err.status === 401 && !isOpenModeNoSession(err)) {
          clearSession();
          setExpired(true);
        }
      })
      .finally(() => {
        // never back to "checking": a re-check after sign-in refreshes the
        // account in the background rather than blanking the shell again
        if (live) setStatus("ready");
      });
    return () => {
      live = false;
    };
  }, [token, clearSession]);

  const value: AuthState = React.useMemo(
    () => ({
      email,
      token,
      user,
      memberships,
      status,
      expired,
      signIn: (e, t = null, u = null) => {
        localStorage.setItem(EMAIL_KEY, e);
        setEmail(e);
        setExpired(false);
        if (u) {
          localStorage.setItem(USER_KEY, JSON.stringify(u));
          setUser(u);
        } else {
          localStorage.removeItem(USER_KEY);
          setUser(null);
          setMemberships([]);
        }
        if (t) {
          localStorage.setItem(TOKEN_KEY, t);
          setToken(t);
        } else {
          localStorage.removeItem(TOKEN_KEY);
          setToken(null);
          setStatus("ready");
        }
      },
      signOut: () => {
        clearSession();
        setExpired(false);
        setStatus("ready");
      },
    }),
    [email, token, user, memberships, status, expired, clearSession],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthState {
  const ctx = React.useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be used within AuthProvider");
  return ctx;
}

/**
 * The session if there is a provider above, `null` if there is not.
 *
 * For components that *offer* something session-related without depending on
 * it — [`LoadError`](../components/LoadError.tsx) shows a "sign in again"
 * button only when signing in is possible. Throwing there would turn a
 * component whose whole job is to explain a failure into a second failure.
 */
export function useOptionalAuth(): AuthState | null {
  return React.useContext(AuthContext);
}
