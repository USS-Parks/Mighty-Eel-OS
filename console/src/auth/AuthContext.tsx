import {
  createContext,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { AogClient, WsfClient } from '../api/client';
import type { TrustToken } from '../api/types';

/** A console session: which stack, the verified trust token, an optional AOG
 *  virtual key for metered gateway endpoints. The token *is* the session. */
export interface Session {
  wsfBase: string;
  aogBase: string;
  token: TrustToken;
  aogKey?: string;
  verifiedAt: string;
}

interface AuthValue {
  session: Session | null;
  /** WSF client bound to the session base URL (null when signed out). */
  wsf: WsfClient | null;
  /** AOG client bound to the session base URL + virtual key. */
  aog: AogClient | null;
  signIn: (session: Session) => void;
  signOut: () => void;
}

const STORAGE_KEY = 'wsf.console.session';
const AuthContext = createContext<AuthValue | undefined>(undefined);

function loadSession(): Session | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? (JSON.parse(raw) as Session) : null;
  } catch {
    return null;
  }
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<Session | null>(loadSession);

  const value = useMemo<AuthValue>(
    () => ({
      session,
      wsf: session ? new WsfClient(session.wsfBase) : null,
      aog: session ? new AogClient(session.aogBase, session.aogKey) : null,
      signIn: (next) => {
        try {
          localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
        } catch {
          // best-effort persistence; the in-memory session still works
        }
        setSession(next);
      },
      signOut: () => {
        try {
          localStorage.removeItem(STORAGE_KEY);
        } catch {
          // ignore
        }
        setSession(null);
      },
    }),
    [session],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthValue {
  const value = useContext(AuthContext);
  if (!value) {
    throw new Error('useAuth must be used within an AuthProvider');
  }
  return value;
}
