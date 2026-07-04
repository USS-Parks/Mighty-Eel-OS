import { Fragment, useCallback, useEffect, useState, type FormEvent } from 'react';
import { useAuth } from '../auth/AuthContext';
import { Panel } from '../components/Panel';
import { StatusPill } from '../components/StatusPill';
import type { LedgerEntry } from '../api/types';

const FIELDS = ['token_id', 'envelope_id', 'subject_hash', 'decision', 'tenant_id'] as const;

function str(rec: Record<string, unknown>, key: string): string {
  const v = rec[key];
  if (typeof v === 'string') return v;
  if (v == null) return '';
  return String(v);
}

/** A one-line op/decision summary from a receipt of any service shape. */
function summary(rec: Record<string, unknown>): string {
  const op = str(rec, 'op');
  const decision = str(rec, 'decision');
  return [op, decision].filter(Boolean).join(' · ') || '—';
}

/** Audit search over the unified WSF receipt ledger (`/v1/receipts`): the one
 *  evidence lake, queryable by correlation field. Bridge issuance + seal/unseal
 *  receipts join here by correlation id. */
export function AuditView() {
  const { wsf } = useAuth();
  const [field, setField] = useState<string>('token_id');
  const [value, setValue] = useState('');
  const [entries, setEntries] = useState<LedgerEntry[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [expanded, setExpanded] = useState<number | null>(null);

  const run = useCallback(
    async (f?: string, v?: string) => {
      if (!wsf) return;
      setLoading(true);
      setErr(null);
      try {
        const r = f && v ? await wsf.receipts(f, v) : await wsf.receipts();
        setEntries(r.entries);
      } catch (e) {
        setErr(e instanceof Error ? e.message : 'query failed');
        setEntries(null);
      } finally {
        setLoading(false);
      }
    },
    [wsf],
  );

  useEffect(() => {
    void run();
  }, [run]);

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    const v = value.trim();
    void run(v ? field : undefined, v || undefined);
  }

  function clear() {
    setValue('');
    void run();
  }

  return (
    <div className="mx-auto max-w-6xl space-y-6">
      <h1 className="text-lg font-semibold text-ink">Audit</h1>

      <Panel title="Search the evidence ledger">
        <form className="flex flex-wrap items-end gap-3" onSubmit={onSubmit}>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="audit-field" className="label-dim">
              Field
            </label>
            <select
              id="audit-field"
              className="field w-44"
              value={field}
              onChange={(e) => setField(e.target.value)}
            >
              {FIELDS.map((f) => (
                <option key={f} value={f}>
                  {f}
                </option>
              ))}
            </select>
          </div>
          <div className="flex flex-1 flex-col gap-1.5">
            <label htmlFor="audit-value" className="label-dim">
              Value
            </label>
            <input
              id="audit-value"
              className="field font-mono"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder="correlation value (blank = all)"
            />
          </div>
          <button type="submit" className="btn btn-signal">
            Search
          </button>
          <button type="button" className="btn" onClick={clear}>
            Clear
          </button>
        </form>
        <p className="mt-3 text-xs text-muted">
          The unified WSF ledger — bridge issuance + seal/unseal receipts, joined by
          correlation id. AOG gateway request receipts are surfaced in Routing &amp; Spend
          and fold into this lake in D4.
        </p>
      </Panel>

      <Panel
        title="Receipts"
        actions={
          entries ? <StatusPill tone="neutral">{entries.length} rows</StatusPill> : undefined
        }
      >
        {loading ? (
          <p className="text-sm text-muted">Searching…</p>
        ) : err ? (
          <p className="text-sm text-bad">Query failed: {err}</p>
        ) : entries && entries.length ? (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead>
                <tr className="label-dim border-b border-edge">
                  <th className="py-2 pr-4 font-medium">Seq</th>
                  <th className="py-2 pr-4 font-medium">Source</th>
                  <th className="py-2 pr-4 font-medium">Token</th>
                  <th className="py-2 pr-4 font-medium">Envelope</th>
                  <th className="py-2 pr-4 font-medium">Op / decision</th>
                  <th className="py-2 font-medium">Raw</th>
                </tr>
              </thead>
              <tbody className="font-mono">
                {entries.map((e) => (
                  <Fragment key={e.seq}>
                    <tr className="border-b border-edge/50">
                      <td className="py-2 pr-4">{e.seq}</td>
                      <td className="py-2 pr-4">{e.source}</td>
                      <td className="py-2 pr-4">{str(e.receipt, 'token_id') || '—'}</td>
                      <td className="py-2 pr-4">{str(e.receipt, 'envelope_id') || '—'}</td>
                      <td className="py-2 pr-4">{summary(e.receipt)}</td>
                      <td className="py-2">
                        <button
                          type="button"
                          className="text-xs text-signal hover:underline"
                          onClick={() => setExpanded(expanded === e.seq ? null : e.seq)}
                        >
                          {expanded === e.seq ? 'hide' : 'view'}
                        </button>
                      </td>
                    </tr>
                    {expanded === e.seq ? (
                      <tr className="border-b border-edge/50">
                        <td colSpan={6} className="bg-base/50 px-4 py-2">
                          <pre className="overflow-x-auto text-xs text-muted">
                            {JSON.stringify(e.receipt, null, 2)}
                          </pre>
                        </td>
                      </tr>
                    ) : null}
                  </Fragment>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="text-sm text-muted">No receipts match.</p>
        )}
      </Panel>
    </div>
  );
}
