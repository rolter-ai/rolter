import * as React from "react";

// client-side session shell only — real auth (local accounts, OAuth2/OIDC SSO)
// lands with the Phase 3 backend and ROL-36. this just gates the UI so the
// control-plane screens sit behind a sign-in surface.
interface AuthState {
  email: string | null;
  signIn: (email: string) => void;
  signOut: () => void;
}

const KEY = "rolter.session.email";
const AuthContext = React.createContext<AuthState | null>(null);

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [email, setEmail] = React.useState<string | null>(() =>
    localStorage.getItem(KEY),
  );

  const value: AuthState = React.useMemo(
    () => ({
      email,
      signIn: (e) => {
        localStorage.setItem(KEY, e);
        setEmail(e);
      },
      signOut: () => {
        localStorage.removeItem(KEY);
        setEmail(null);
      },
    }),
    [email],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthState {
  const ctx = React.useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be used within AuthProvider");
  return ctx;
}
