import { useEffect, useState } from 'react';
import { useAuth } from '../auth/AuthContext';
import { Field } from '../components/Field';
import { Metric } from '../components/Metric';
import { Panel } from '../components/Panel';
import { StatusPill, type PillTone } from '../components/StatusPill';
import type { StatusResp } from '../api/types';

const MODE_LABEL: Record<StatusResp['mode'], string> = {
  shadow: 'shadow',
  report_only: 'report-only',
  enforce: 'enforce',
};
const MODE_TONE: Record<StatusResp['mode'], PillTone> = {
  shadow: 'warn',
  report_only: 'signal',
  enforce: 'ok',
};

function usd(cents: number): string {
  return `$${(cents / 100).toFixed(2)}`;
}

function ratio(spent: number, cap: number, fmt: (n: number) => string): string {
  return cap > 0 ? `${fmt(spent)} / ${fmt(cap)}` : `${fmt(spent)} / ∞`;
}

function boolLabel(value: boolean | null): string {
  if (value === null) return '…';
  return value ? 'up' : 'down';
}

function shortHash(hash: string): string {
  return hash.length > 12 ? `${hash.slice(0, 10)}…` : hash;
}

/** Join a (possibly absent) string list for display, or an em dash. Defensive:
 *  a pasted token may omit the serde-default arrays. */
function list(items: string[] | null | undefined): string {
  return items && items.length ? items.join(', ') : '—';
}

/** Overview + trust status — the ported, live version of the Jinja2 overview:
 *  enforcement mode, connectivity, audit-chain integrity, tenant + trust bundle
 *  + budget, and the gateway's advertised capability. */
export function OverviewView() {
  const { session, wsf, aog } = useAuth();
  const token = session?.token ?? null;
  const [wsfUp, setWsfUp] = useState<boolean | null>(null);
  const [aogUp, setAogUp] = useState<boolean | null>(null);
  const [status, setStatus] = useState<StatusResp | null>(null);
  const [statusErr, setStatusErr] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    async function load() {
      const [w, a] = await Promise.all([
        wsf ? wsf.health() : Promise.resolve(false),
        aog ? aog.health() : Promise.resolve(false),
      ]);
      if (!alive) return;
      setWsfUp(w);
      setAogUp(a);
      try {
        const s = aog ? await aog.status() : null;
        if (alive) {
          setStatus(s);
          setStatusErr(null);
        }
      } catch (err) {
        if (alive) {
          setStatus(null);
          setStatusErr(err instanceof Error ? err.message : 'unreachable');
        }
      }
    }
    void load();
    return () => {
      alive = false;
    };
  }, [wsf, aog]);

  const connectivity: { label: string; tone: PillTone } =
    wsfUp && aogUp
      ? { label: 'Connected', tone: 'ok' }
      : wsfUp || aogUp
        ? { label: 'Degraded', tone: 'warn' }
        : { label: 'Unreachable', tone: 'bad' };

  return (
    <div className="mx-auto max-w-6xl space-y-6">
      <h1 className="text-lg font-semibold text-ink">Overview</h1>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <Metric
          label="Trust mode"
          value={
            status ? (
              <StatusPill tone={MODE_TONE[status.mode]}>{MODE_LABEL[status.mode]}</StatusPill>
            ) : (
              <StatusPill tone="neutral">{statusErr ? 'unknown' : '…'}</StatusPill>
            )
          }
          hint={
            status
              ? 'AOG enforcement posture'
              : statusErr
                ? 'gateway unreachable'
                : 'querying gateway'
          }
        />
        <Metric
          label="Connectivity"
          value={<StatusPill tone={connectivity.tone}>{connectivity.label}</StatusPill>}
          hint={`WSF ${boolLabel(wsfUp)} · AOG ${boolLabel(aogUp)}`}
        />
        <Metric
          label="Audit chain"
          value={
            status ? (
              <StatusPill tone={status.chain_verified ? 'ok' : 'bad'}>
                {status.chain_verified ? 'Verified' : 'BROKEN'}
              </StatusPill>
            ) : (
              <StatusPill tone="neutral">unknown</StatusPill>
            )
          }
          hint={status ? `${status.receipts} receipts · head ${shortHash(status.chain_head)}` : '—'}
        />
      </div>

      <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
        <Panel title="Tenant">
          {token ? (
            <div>
              <Field label="Tenant">{token.tenant_id}</Field>
              <Field label="Roles">{list(token.roles)}</Field>
              <Field label="Compliance scopes">{list(token.compliance_scopes)}</Field>
              <Field label="Allowed routes">{list(token.allowed_routes)}</Field>
              <Field label="Max classification">{token.max_data_classification}</Field>
            </div>
          ) : (
            <p className="text-sm text-muted">No session token.</p>
          )}
        </Panel>

        <Panel title="Trust bundle">
          {token ? (
            <div>
              <Field label="Bundle version">{token.trust_bundle_version}</Field>
              <Field label="Issuer">{token.issuer}</Field>
              <Field label="Token id">{token.token_id}</Field>
              <Field label="Issued">{token.issued_at}</Field>
              <Field label="Expires">{token.expires_at}</Field>
            </div>
          ) : (
            <p className="text-sm text-muted">No session token.</p>
          )}
        </Panel>

        <Panel title="Budget">
          {token?.budget ? (
            <div>
              <Field label="Tokens">
                {ratio(token.budget.tokens_spent, token.budget.token_cap, String)}
              </Field>
              <Field label="USD">
                {ratio(token.budget.usd_spent_cents, token.budget.usd_cap_cents, usd)}
              </Field>
              <Field label="Tool calls">
                {ratio(token.budget.tool_calls_spent, token.budget.tool_call_cap, String)}
              </Field>
            </div>
          ) : (
            <p className="text-sm text-muted">
              No budget strand on this token (enforcement off).
            </p>
          )}
        </Panel>

        <Panel title="Gateway capability">
          {status ? (
            <div>
              <Field label="Providers">{list(status.providers)}</Field>
              <Field label="Models">{list(status.models)}</Field>
              <Field label="Receipts">{String(status.receipts)}</Field>
            </div>
          ) : (
            <p className="text-sm text-muted">
              {statusErr ? `Gateway unreachable: ${statusErr}` : 'Querying gateway…'}
            </p>
          )}
        </Panel>
      </div>
    </div>
  );
}
