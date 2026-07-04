import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { RoutingView } from '../views/RoutingView';
import { AuthProvider } from '../auth/AuthContext';

function ok(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as unknown as Response;
}

const TOKEN = {
  token_id: 'tok_r',
  tenant_id: 'acme',
  issuer: 'wsf-bridge',
  trust_bundle_version: 'b1',
  issued_at: '2026-07-03T00:00:00Z',
  expires_at: '2026-07-04T00:00:00Z',
  subject_hash: 'h',
  roles: [],
  compliance_scopes: [],
  allowed_routes: [],
  allowed_models: [],
  max_data_classification: 'public',
  offline_mode: false,
  revocation_status: 'valid',
  attenuation: { caveats: [] },
  signature: { alg: 'ml-dsa-87', key_id: 'k', value: 'v' },
};

const SESSION = {
  wsfBase: 'http://wsf',
  aogBase: 'http://aog',
  aogKey: 'vk_test',
  token: TOKEN,
  verifiedAt: '2026-07-03T00:00:00Z',
};

const USAGE = {
  aggregates: [
    {
      tenant_id: 'acme',
      provider: 'local',
      model: 'llama3',
      workflow_id: 'task-a',
      calls: 3,
      input_tokens: 300,
      output_tokens: 150,
      spend_cents: 0,
    },
    {
      tenant_id: 'acme',
      provider: 'openai',
      model: 'gpt-4o-mini',
      workflow_id: 'task-a',
      calls: 2,
      input_tokens: 200,
      output_tokens: 100,
      spend_cents: 45,
    },
  ],
  chain_head: 'abcd',
  chain_verified: true,
};

function renderView() {
  return render(
    <AuthProvider>
      <RoutingView />
    </AuthProvider>,
  );
}

beforeEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('RoutingView', () => {
  it('renders routing + spend charts from the live meter', async () => {
    localStorage.setItem('wsf.console.session', JSON.stringify(SESSION));
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string) =>
        Promise.resolve(url.endsWith('/v1/usage') ? ok(USAGE) : ok({})),
      ),
    );
    renderView();

    // Total cloud spend (metric + provider bar + table row all show $0.45).
    expect((await screen.findAllByText('$0.45')).length).toBeGreaterThan(0);
    // Per-model row from the aggregate.
    expect(screen.getByText('gpt-4o-mini')).toBeInTheDocument();
    // Task label appears (two rows share task-a).
    expect(screen.getAllByText('task-a').length).toBeGreaterThan(0);
    // The local-vs-cloud routing chart.
    expect(screen.getByText('3 calls')).toBeInTheDocument();
    expect(screen.getByText('2 calls')).toBeInTheDocument();
  });

  it('prompts for a virtual key when the session has none', async () => {
    const noKey: Record<string, unknown> = { ...SESSION };
    delete noKey.aogKey;
    localStorage.setItem('wsf.console.session', JSON.stringify(noKey));
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(ok({}))),
    );
    renderView();

    expect(await screen.findByText(/virtual key/i)).toBeInTheDocument();
  });
});
