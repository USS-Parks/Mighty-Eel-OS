import { useCallback, useEffect, useState } from 'react';
import { useAuth } from '../auth/AuthContext';
import { Panel } from '../components/Panel';
import { StatusPill } from '../components/StatusPill';
import type { PillTone } from '../components/StatusPill';
import type { SessionResp, SessionStepKind, SessionSummary } from '../api/types';

function kindTone(kind: SessionStepKind): PillTone {
  switch (kind) {
    case 'prompt':
      return 'signal';
    case 'tool_call':
      return 'warn';
    case 'approval':
      return 'ok';
    default:
      return 'neutral';
  }
}

/** Session replay (C7) — a T7 agent session rendered as a replayable trace. Pick a
 *  recorded session and step through it: prompt → model turn → tool call → approval
 *  → tool result, one step at a time. The trace is verified from the ledger, so
 *  what you replay is exactly what ran. */
export function SessionReplayView() {
  const { aog } = useAuth();
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [session, setSession] = useState<SessionResp | null>(null);
  const [cursor, setCursor] = useState(0);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    if (!aog) return;
    void (async () => {
      try {
        const r = await aog.sessions();
        setSessions(r.sessions);
        if (r.sessions.length && selectedId === null) {
          setSelectedId(r.sessions[0].session_id);
        }
      } catch (e) {
        setErr(e instanceof Error ? e.message : 'failed to load sessions');
      }
    })();
    // Only re-list when the client changes; selection is handled separately.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [aog]);

  const loadSession = useCallback(
    async (id: string) => {
      if (!aog) return;
      setErr(null);
      try {
        const r = await aog.session(id);
        setSession(r);
        setCursor(0);
      } catch (e) {
        setErr(e instanceof Error ? e.message : 'failed to load the session');
        setSession(null);
      }
    },
    [aog],
  );

  useEffect(() => {
    if (selectedId) void loadSession(selectedId);
  }, [selectedId, loadSession]);

  const steps = session?.steps ?? [];
  const total = steps.length;
  const current = total ? steps[Math.min(cursor, total - 1)] : null;

  return (
    <div className="mx-auto max-w-4xl space-y-6">
      <h1 className="text-lg font-semibold text-ink">Session replay</h1>

      <Panel title="Recorded sessions">
        {err ? (
          <p className="text-sm text-bad">Error: {err}</p>
        ) : sessions === null ? (
          <p className="text-sm text-muted">Loading sessions…</p>
        ) : sessions.length ? (
          <div className="flex flex-wrap items-end gap-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="sess-pick" className="label-dim">
                Session
              </label>
              <select
                id="sess-pick"
                className="field w-72 font-mono"
                value={selectedId ?? ''}
                onChange={(e) => setSelectedId(e.target.value)}
              >
                {sessions.map((s) => (
                  <option key={s.session_id} value={s.session_id}>
                    {s.session_id} ({s.steps} steps)
                  </option>
                ))}
              </select>
            </div>
          </div>
        ) : (
          <p className="text-sm text-muted">No recorded sessions yet.</p>
        )}
      </Panel>

      {session ? (
        <Panel
          title="Replay"
          actions={
            <div className="flex items-center gap-2">
              <StatusPill tone={session.chain_verified ? 'ok' : 'bad'}>
                {session.chain_verified ? 'Verified' : 'Chain broken'}
              </StatusPill>
              <StatusPill tone="neutral">
                {`Step ${total ? Math.min(cursor + 1, total) : 0} of ${total}`}
              </StatusPill>
            </div>
          }
        >
          <div className="mb-4 flex gap-2">
            <button
              type="button"
              className="btn"
              disabled={cursor <= 0}
              onClick={() => setCursor((c) => Math.max(0, c - 1))}
            >
              ‹ Prev
            </button>
            <button
              type="button"
              className="btn btn-signal"
              disabled={cursor >= total - 1}
              onClick={() => setCursor((c) => Math.min(total - 1, c + 1))}
            >
              Next ›
            </button>
            <button type="button" className="btn" onClick={() => setCursor(0)}>
              Restart
            </button>
          </div>

          {current ? (
            <div className="mb-5 rounded-lg border border-signal/40 bg-signal/5 p-4">
              <div className="flex items-center gap-2">
                <StatusPill tone={kindTone(current.kind)}>{current.kind}</StatusPill>
                {current.actor ? (
                  <span className="font-mono text-xs text-muted">{current.actor}</span>
                ) : null}
                <span className="ml-auto font-mono text-[11px] text-muted">{current.at}</span>
              </div>
              <p className="mt-2 text-sm text-ink">{current.summary}</p>
            </div>
          ) : null}

          <ol className="space-y-2">
            {steps.map((s, i) => (
              <li
                key={s.seq}
                className={`flex items-center gap-3 rounded border px-3 py-2 text-sm transition ${
                  i === cursor
                    ? 'border-signal/50 bg-signal/10 text-ink'
                    : i < cursor
                      ? 'border-edge text-muted'
                      : 'border-edge/50 text-muted opacity-50'
                }`}
              >
                <span className="w-6 shrink-0 text-right font-mono text-xs">{s.seq}</span>
                <StatusPill tone={kindTone(s.kind)}>{s.kind}</StatusPill>
                <span className="min-w-0 flex-1 truncate">{s.summary}</span>
              </li>
            ))}
          </ol>
        </Panel>
      ) : null}
    </div>
  );
}
