import { useCallback, useEffect, useState, type FormEvent } from 'react';
import { useAuth } from '../auth/AuthContext';
import { Panel } from '../components/Panel';
import { StatusPill } from '../components/StatusPill';
import type { DenialExplanation, PolicyMode, PolicyRule } from '../api/types';

const MODES: { mode: PolicyMode; label: string }[] = [
  { mode: 'shadow', label: 'Shadow' },
  { mode: 'report_only', label: 'Report' },
  { mode: 'enforce', label: 'Enforce' },
];

function modeTone(mode: PolicyMode): 'neutral' | 'warn' | 'bad' {
  if (mode === 'enforce') return 'bad';
  if (mode === 'report_only') return 'warn';
  return 'neutral';
}

/** Policy Studio (C6) — policy-as-code, rendered in plain language, with a
 *  shadow → report → enforce toggle per rule, and the killer demo: paste a denied
 *  decision and get a human-readable explanation that cites the exact policy line. */
export function PolicyStudioView() {
  const { aog } = useAuth();
  const [rules, setRules] = useState<PolicyRule[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const [query, setQuery] = useState('');
  const [explaining, setExplaining] = useState(false);
  const [explanation, setExplanation] = useState<DenialExplanation | null>(null);
  const [explainErr, setExplainErr] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!aog) return;
    setErr(null);
    try {
      const r = await aog.policy();
      setRules(r.rules);
    } catch (e) {
      setErr(e instanceof Error ? e.message : 'failed to load the policy');
      setRules(null);
    }
  }, [aog]);

  useEffect(() => {
    void load();
  }, [load]);

  async function setMode(rule: PolicyRule, mode: PolicyMode) {
    if (!aog || rule.mode === mode) return;
    setBusy(rule.id);
    try {
      const updated = await aog.setPolicyMode(rule.id, mode);
      setRules((prev) =>
        prev ? prev.map((r) => (r.id === rule.id ? { ...r, mode: updated.mode } : r)) : prev,
      );
    } catch (e) {
      setErr(e instanceof Error ? e.message : 'failed to change the mode');
    } finally {
      setBusy(null);
    }
  }

  async function explain(e: FormEvent) {
    e.preventDefault();
    if (!aog) return;
    const q = query.trim();
    if (!q) return;
    setExplaining(true);
    setExplainErr(null);
    setExplanation(null);
    try {
      setExplanation(await aog.explainDenial(q));
    } catch (er) {
      setExplainErr(er instanceof Error ? er.message : 'explanation failed');
    } finally {
      setExplaining(false);
    }
  }

  return (
    <div className="mx-auto max-w-4xl space-y-6">
      <h1 className="text-lg font-semibold text-ink">Policy studio</h1>

      <Panel title="Explain a denial">
        <form className="flex flex-wrap items-end gap-3" onSubmit={explain}>
          <div className="flex flex-1 flex-col gap-1.5">
            <label htmlFor="explain-q" className="label-dim">
              Denied decision or correlation id
            </label>
            <input
              id="explain-q"
              className="field font-mono"
              value={query}
              onChange={(ev) => setQuery(ev.target.value)}
              placeholder="e.g. a receipt id, or a decision correlation"
            />
          </div>
          <button type="submit" className="btn btn-signal" disabled={explaining}>
            {explaining ? 'Explaining…' : 'Explain'}
          </button>
        </form>

        {explainErr ? (
          <p className="mt-3 text-sm text-bad">Explanation failed: {explainErr}</p>
        ) : explanation ? (
          <div className="mt-4 space-y-2 rounded-lg border border-edge bg-base/40 p-4">
            <div className="flex items-center gap-2">
              <StatusPill tone="bad">Denied</StatusPill>
              <span className="text-sm font-semibold text-ink">{explanation.rule_title}</span>
            </div>
            <p className="text-sm text-muted">{explanation.plain_language}</p>
            <div>
              <div className="label-dim mb-1">Cited policy line ({explanation.rule_id})</div>
              <pre className="overflow-x-auto rounded border border-edge bg-panel px-3 py-2 text-xs text-signal">
                {explanation.cited_line}
              </pre>
            </div>
          </div>
        ) : (
          <p className="mt-3 text-xs text-muted">
            A local model explains the denial in plain language and cites the exact rule
            line — no cloud call, the explanation stays inside your boundary.
          </p>
        )}
      </Panel>

      <Panel title="Policy as code">
        {err ? (
          <p className="text-sm text-bad">Policy error: {err}</p>
        ) : rules === null ? (
          <p className="text-sm text-muted">Loading the policy…</p>
        ) : rules.length ? (
          <ul className="space-y-4">
            {rules.map((rule) => (
              <li key={rule.id} className="rounded-lg border border-edge bg-base/40 p-4">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="text-sm font-semibold text-ink">{rule.title}</span>
                      <StatusPill tone="neutral">{rule.regime}</StatusPill>
                      <StatusPill tone={modeTone(rule.mode)}>{rule.mode}</StatusPill>
                    </div>
                    <p className="mt-1 text-sm text-muted">{rule.plain_language}</p>
                  </div>
                  <div className="flex shrink-0 gap-1" role="group" aria-label={`mode for ${rule.title}`}>
                    {MODES.map((m) => (
                      <button
                        key={m.mode}
                        type="button"
                        className={`btn ${rule.mode === m.mode ? 'btn-signal' : ''}`}
                        disabled={busy === rule.id}
                        onClick={() => void setMode(rule, m.mode)}
                      >
                        {m.label}
                      </button>
                    ))}
                  </div>
                </div>
                <pre className="mt-3 overflow-x-auto rounded border border-edge bg-panel px-3 py-2 text-xs text-muted">
                  {rule.code}
                </pre>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-muted">No policy rules loaded.</p>
        )}
      </Panel>
    </div>
  );
}
