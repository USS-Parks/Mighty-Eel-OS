import type { ReactNode } from 'react';
import { NavLink } from 'react-router-dom';
import { useAuth } from '../auth/AuthContext';
import { StatusPill } from './StatusPill';

const NAV = [
  { to: '/', label: 'Overview', end: true },
  { to: '/routing', label: 'Routing & Spend', end: false },
  { to: '/audit', label: 'Audit', end: false },
];

function shortId(id: string): string {
  return id.length > 14 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id;
}

function Brand() {
  return (
    <div className="flex items-center gap-2.5 px-4 py-4">
      <svg viewBox="0 0 32 32" className="h-7 w-7 text-signal" aria-hidden="true">
        <path
          d="M5 25 L16 6 L27 25 Z"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.2"
          strokeLinejoin="round"
        />
        <circle cx="16" cy="19.5" r="2.4" fill="currentColor" />
      </svg>
      <div className="leading-tight">
        <div className="text-sm font-semibold text-ink">Sovereignty</div>
        <div className="text-[11px] uppercase tracking-widest text-muted">Console</div>
      </div>
    </div>
  );
}

/** App frame: left nav rail + top session bar around the routed content. */
export function AppShell({ children }: { children: ReactNode }) {
  const { session, signOut } = useAuth();
  const tenant = session?.token.tenant_id ?? '—';
  const tokenId = session?.token.token_id ?? '—';

  return (
    <div className="flex h-full">
      <aside className="flex w-60 shrink-0 flex-col border-r border-edge bg-panel">
        <Brand />
        <nav className="flex flex-1 flex-col gap-1 px-3 py-2">
          {NAV.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.end}
              className={({ isActive }) =>
                `rounded-lg px-3 py-2 text-sm font-medium transition ${
                  isActive
                    ? 'bg-signal/10 text-signal'
                    : 'text-muted hover:bg-white/5 hover:text-ink'
                }`
              }
            >
              {item.label}
            </NavLink>
          ))}
        </nav>
        <div className="border-t border-edge px-4 py-3">
          <StatusPill tone="warn">Shadow mode</StatusPill>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex items-center justify-between border-b border-edge bg-panel px-6 py-3">
          <div className="text-sm text-muted">Island Mountain · WSF + AOG</div>
          <div className="flex items-center gap-4">
            <div className="text-right leading-tight">
              <div className="text-[11px] uppercase tracking-wider text-muted">tenant</div>
              <div className="font-mono text-sm text-ink">{tenant}</div>
            </div>
            <div className="hidden text-right leading-tight sm:block">
              <div className="text-[11px] uppercase tracking-wider text-muted">token</div>
              <div className="font-mono text-xs text-ink">{shortId(tokenId)}</div>
            </div>
            <button type="button" className="btn" onClick={signOut}>
              Sign out
            </button>
          </div>
        </header>
        <main className="flex-1 overflow-auto p-6">{children}</main>
      </div>
    </div>
  );
}
