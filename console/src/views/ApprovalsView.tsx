import { useCallback, useEffect, useState } from 'react';
import { useAuth } from '../auth/AuthContext';
import { Panel } from '../components/Panel';
import { StatusPill } from '../components/StatusPill';
import type { ApprovalTicket } from '../api/types';

/** The approval inbox (C5) — the T3 human-in-the-loop gate. Side-effecting tool
 *  calls (and any call an untrusted-provenance or out-of-contract path escalated)
 *  pause here with a diff preview; an operator approves or denies, and the
 *  decision + actor are receipted. The same inbox WSF cred grants and Aeneas
 *  remediations use. */
export function ApprovalsView() {
  const { aog, session } = useAuth();
  const actor = session?.token.token_id ?? 'console-operator';

  const [pending, setPending] = useState<ApprovalTicket[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [acting, setActing] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!aog) return;
    setLoading(true);
    setErr(null);
    try {
      const r = await aog.approvals();
      setPending(r.pending);
    } catch (e) {
      setErr(e instanceof Error ? e.message : 'failed to load the inbox');
      setPending(null);
    } finally {
      setLoading(false);
    }
  }, [aog]);

  useEffect(() => {
    void load();
  }, [load]);

  async function decide(ticket: ApprovalTicket, approve: boolean) {
    if (!aog) return;
    setActing(ticket.id);
    setNotice(null);
    try {
      const res = approve
        ? await aog.approve(ticket.id, actor)
        : await aog.deny(ticket.id, actor, 'denied via console');
      setNotice(`${ticket.tool_id} ${res.status} by ${res.actor}`);
      await load();
    } catch (e) {
      setErr(e instanceof Error ? e.message : 'the decision failed');
    } finally {
      setActing(null);
    }
  }

  const count = pending?.length ?? 0;

  return (
    <div className="mx-auto max-w-4xl space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold text-ink">Approval inbox</h1>
        <button type="button" className="btn" onClick={() => void load()}>
          Refresh
        </button>
      </div>

      {notice ? (
        <Panel>
          <p className="text-sm text-signal">{notice}</p>
        </Panel>
      ) : null}

      <Panel
        title="Pending approvals"
        actions={
          <StatusPill tone={count ? 'warn' : 'ok'}>{count} pending</StatusPill>
        }
      >
        {loading && pending === null ? (
          <p className="text-sm text-muted">Loading the inbox…</p>
        ) : err ? (
          <p className="text-sm text-bad">Inbox error: {err}</p>
        ) : pending && pending.length ? (
          <ul className="space-y-4">
            {pending.map((t) => (
              <li key={t.id} className="rounded-lg border border-edge bg-base/40 p-4">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="font-mono text-sm text-ink">{t.tool_id}</div>
                    <div className="mt-0.5 text-sm text-muted">{t.summary}</div>
                    <div className="mt-1 text-[11px] uppercase tracking-wider text-muted">
                      session {t.session_id} · token {t.profile_id} · {t.requested_at}
                    </div>
                  </div>
                  <div className="flex shrink-0 gap-2">
                    <button
                      type="button"
                      className="btn btn-signal"
                      disabled={acting === t.id}
                      onClick={() => void decide(t, true)}
                    >
                      Approve
                    </button>
                    <button
                      type="button"
                      className="btn"
                      disabled={acting === t.id}
                      onClick={() => void decide(t, false)}
                    >
                      Deny
                    </button>
                  </div>
                </div>
                <pre className="mt-3 overflow-x-auto rounded border border-edge bg-panel px-3 py-2 text-xs text-muted">
                  {t.diff_preview}
                </pre>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-muted">No pending approvals — the queue is clear.</p>
        )}
      </Panel>

      <p className="text-xs text-muted">
        Every decision — approve or deny, with the actor — is receipted into the same
        tamper-evident ledger the tool calls chain into (T3). A denied call never executes.
      </p>
    </div>
  );
}
