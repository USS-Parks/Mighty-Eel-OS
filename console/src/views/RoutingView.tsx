import { useEffect, useState } from 'react';
import { useAuth } from '../auth/AuthContext';
import { BarChart, type Bar } from '../components/BarChart';
import { Metric } from '../components/Metric';
import { Panel } from '../components/Panel';
import type { TaskUsage, UsageResp } from '../api/types';

function usd(cents: number): string {
  return `$${(cents / 100).toFixed(2)}`;
}
function sum(nums: number[]): number {
  return nums.reduce((a, b) => a + b, 0);
}

/** Routing + spend dashboards, driven by the AOG `/v1/usage` meter (G7). Live
 *  routing (local vs cloud, per provider), spend, and the sovereignty dividend. */
export function RoutingView() {
  const { session, aog } = useAuth();
  const hasKey = Boolean(session?.aogKey);
  const [usage, setUsage] = useState<UsageResp | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(hasKey);

  useEffect(() => {
    if (!aog || !hasKey) return;
    let alive = true;
    setLoading(true);
    aog
      .usage()
      .then((u) => {
        if (alive) {
          setUsage(u);
          setErr(null);
        }
      })
      .catch((e: unknown) => {
        if (alive) setErr(e instanceof Error ? e.message : 'failed to load metering');
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [aog, hasKey]);

  return (
    <div className="mx-auto max-w-6xl space-y-6">
      <h1 className="text-lg font-semibold text-ink">Routing &amp; Spend</h1>
      {!hasKey ? (
        <Panel title="Metering">
          <p className="text-sm text-muted">
            Metering is read from the gateway with your AOG virtual key. Sign out and sign
            back in with a virtual key to view routing and spend.
          </p>
        </Panel>
      ) : loading ? (
        <Panel title="Metering">
          <p className="text-sm text-muted">Loading metering…</p>
        </Panel>
      ) : err ? (
        <Panel title="Metering">
          <p className="text-sm text-bad">Could not load metering: {err}</p>
        </Panel>
      ) : usage ? (
        <Dashboards usage={usage} />
      ) : null}
    </div>
  );
}

function Dashboards({ usage }: { usage: UsageResp }) {
  const agg = usage.aggregates;
  const isLocal = (t: TaskUsage) => t.provider === 'local';
  const cloudSpend = sum(agg.map((a) => a.spend_cents));
  const localCalls = sum(agg.filter(isLocal).map((a) => a.calls));
  const cloudCalls = sum(agg.filter((a) => !isLocal(a)).map((a) => a.calls));

  const routingBars: Bar[] = [
    { label: 'Local', value: localCalls, display: `${localCalls} calls`, tone: 'ok' },
    { label: 'Cloud', value: cloudCalls, display: `${cloudCalls} calls`, tone: 'signal' },
  ];

  const providerSpend = new Map<string, number>();
  for (const a of agg) {
    providerSpend.set(a.provider, (providerSpend.get(a.provider) ?? 0) + a.spend_cents);
  }
  const spendBars: Bar[] = [...providerSpend.entries()]
    .sort((a, b) => b[1] - a[1])
    .map(([provider, cents]) => ({
      label: provider,
      value: cents,
      display: usd(cents),
      tone: provider === 'local' ? ('ok' as const) : ('signal' as const),
    }));

  return (
    <div className="space-y-6">
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <Metric label="Cloud spend" value={usd(cloudSpend)} hint="metered, to date" />
        <Metric label="Local calls" value={String(localCalls)} hint="served on-prem at $0" />
        <Metric label="Cloud calls" value={String(cloudCalls)} hint="billed by provider" />
      </div>

      <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
        <Panel title="Routing — local vs cloud">
          <BarChart bars={routingBars} />
        </Panel>
        <Panel title="Spend by provider">
          <BarChart bars={spendBars} />
        </Panel>
      </div>

      <Panel title="Usage by task">
        {agg.length ? (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead>
                <tr className="label-dim border-b border-edge">
                  <th className="py-2 pr-4 font-medium">Task</th>
                  <th className="py-2 pr-4 font-medium">Provider</th>
                  <th className="py-2 pr-4 font-medium">Model</th>
                  <th className="py-2 pr-4 text-right font-medium">Calls</th>
                  <th className="py-2 pr-4 text-right font-medium">In</th>
                  <th className="py-2 pr-4 text-right font-medium">Out</th>
                  <th className="py-2 text-right font-medium">Spend</th>
                </tr>
              </thead>
              <tbody className="font-mono">
                {agg.map((a, i) => (
                  <tr
                    key={`${a.workflow_id ?? ''}-${a.provider}-${a.model}-${i}`}
                    className="border-b border-edge/50 last:border-0"
                  >
                    <td className="py-2 pr-4">{a.workflow_id || '—'}</td>
                    <td className="py-2 pr-4">{a.provider}</td>
                    <td className="py-2 pr-4">{a.model}</td>
                    <td className="py-2 pr-4 text-right">{a.calls}</td>
                    <td className="py-2 pr-4 text-right">{a.input_tokens}</td>
                    <td className="py-2 pr-4 text-right">{a.output_tokens}</td>
                    <td className="py-2 text-right">{usd(a.spend_cents)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="text-sm text-muted">No metered requests yet.</p>
        )}
      </Panel>

      <Panel title="ROI">
        <p className="text-sm text-muted">
          Local requests are served on-prem at $0 — {localCalls} so far. The automated
          break-even recommender (idle → move on-prem, saturation → upgrade) is G10, landing
          in M2; this M1 view surfaces the live meter it builds on.
        </p>
      </Panel>
    </div>
  );
}
