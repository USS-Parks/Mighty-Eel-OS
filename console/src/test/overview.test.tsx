import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { BrowserRouter } from 'react-router-dom';
import App from '../App';
import { AuthProvider } from '../auth/AuthContext';

function ok(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => (typeof body === 'string' ? body : JSON.stringify(body)),
  } as unknown as Response;
}

const SESSION = {
  wsfBase: 'http://wsf',
  aogBase: 'http://aog',
  token: {
    token_id: 'tok_over_9999',
    tenant_id: 'acme-health',
    issuer: 'wsf-bridge',
    trust_bundle_version: 'bundle-7',
    issued_at: '2026-07-03T00:00:00Z',
    expires_at: '2026-07-04T00:00:00Z',
    subject_hash: 'h',
    roles: ['clinician'],
    compliance_scopes: ['hipaa'],
    allowed_routes: ['local_only', 'cloud_allowed'],
    allowed_models: ['gpt-4o'],
    max_data_classification: 'controlled',
    offline_mode: false,
    revocation_status: 'valid',
    budget: {
      token_cap: 1000,
      tokens_spent: 250,
      usd_cap_cents: 5000,
      usd_spent_cents: 1200,
      tool_call_cap: 0,
      tool_calls_spent: 0,
    },
    attenuation: { caveats: [] },
    signature: { alg: 'ml-dsa-87', key_id: 'k', value: 'v' },
  },
  verifiedAt: '2026-07-03T00:00:00Z',
};

function routeFetch(url: string): Response {
  if (url.endsWith('/v1/status')) {
    return ok({
      mode: 'shadow',
      providers: ['local', 'openai'],
      models: ['gpt-4o'],
      receipts: 4,
      chain_head: 'abcdef0123456789',
      chain_verified: true,
    });
  }
  if (url.endsWith('/healthz')) return ok('ok');
  return ok({});
}

function renderApp() {
  return render(
    <BrowserRouter>
      <AuthProvider>
        <App />
      </AuthProvider>
    </BrowserRouter>,
  );
}

beforeEach(() => {
  localStorage.clear();
  localStorage.setItem('wsf.console.session', JSON.stringify(SESSION));
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.stubGlobal(
    'fetch',
    vi.fn((url: string) => Promise.resolve(routeFetch(url))),
  );
});

describe('OverviewView', () => {
  it('renders live trust status from the stack', async () => {
    renderApp();
    // Tenant summary from the session token (appears in top bar + panel).
    expect((await screen.findAllByText('acme-health')).length).toBeGreaterThan(0);
    expect(screen.getByText('bundle-7')).toBeInTheDocument();
    // Trust mode + audit-chain integrity from /v1/status.
    expect(await screen.findByText('shadow')).toBeInTheDocument();
    expect(screen.getByText('Verified')).toBeInTheDocument();
    // Gateway capability.
    expect(screen.getByText('local, openai')).toBeInTheDocument();
  });

  it('shows Unreachable when the whole stack is down', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.reject(new Error('down'))),
    );
    renderApp();
    expect(await screen.findByText('Unreachable')).toBeInTheDocument();
  });
});
