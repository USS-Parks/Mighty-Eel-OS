import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { PolicyStudioView } from '../views/PolicyStudioView';
import { AuthProvider } from '../auth/AuthContext';

function ok(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as unknown as Response;
}

const SESSION = {
  wsfBase: 'http://wsf',
  aogBase: 'http://aog',
  aogKey: 'vk_demo',
  token: { token_id: 't', tenant_id: 'acme' },
  verifiedAt: '2026-07-04T00:00:00Z',
};

const RULE = {
  id: 'hipaa-phi-local',
  title: 'PHI stays local',
  regime: 'hipaa',
  plain_language: 'Protected health information may not leave the box.',
  code: 'when classification == phi: route = local_only',
  mode: 'shadow',
};

const EXPLANATION = {
  decision: 'dec_1',
  rule_id: 'hipaa-phi-local',
  rule_title: 'PHI stays local',
  cited_line: 'when classification == phi: route = local_only',
  plain_language:
    'This request carried PHI, which policy pins to local-only routing, so the cloud destination was denied.',
};

function routed(url: string): Response {
  if (url.includes('/v1/policy/explain')) return ok(EXPLANATION);
  if (url.includes('/mode')) return ok({ ...RULE, mode: 'enforce' });
  if (url.includes('/v1/policy')) return ok({ rules: [RULE], default_mode: 'shadow' });
  return ok({});
}

function renderView() {
  return render(
    <AuthProvider>
      <PolicyStudioView />
    </AuthProvider>,
  );
}

beforeEach(() => {
  localStorage.clear();
  localStorage.setItem('wsf.console.session', JSON.stringify(SESSION));
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('PolicyStudioView', () => {
  it('renders policy rules in plain language + code', async () => {
    vi.stubGlobal('fetch', vi.fn((url: string) => Promise.resolve(routed(url))));
    renderView();

    expect(await screen.findByText('PHI stays local')).toBeInTheDocument();
    expect(
      screen.getByText('Protected health information may not leave the box.'),
    ).toBeInTheDocument();
    // The policy-as-code line is shown.
    expect(
      screen.getAllByText(/when classification == phi/).length,
    ).toBeGreaterThan(0);
  });

  it('explains a denial with a human-readable, rule-cited explanation (the gate)', async () => {
    vi.stubGlobal('fetch', vi.fn((url: string) => Promise.resolve(routed(url))));
    renderView();
    await screen.findByText('PHI stays local'); // rules loaded

    fireEvent.change(screen.getByLabelText(/denied decision/i), {
      target: { value: 'dec_1' },
    });
    fireEvent.click(screen.getByRole('button', { name: /explain/i }));

    // Human-readable explanation + the exact cited policy line + the rule id.
    expect(
      await screen.findByText(/pins to local-only routing, so the cloud destination was denied/),
    ).toBeInTheDocument();
    expect(screen.getByText(/Cited policy line \(hipaa-phi-local\)/)).toBeInTheDocument();
  });

  it('a mode toggle drives the set-mode endpoint', async () => {
    const fetchMock = vi.fn((url: string) => Promise.resolve(routed(url)));
    vi.stubGlobal('fetch', fetchMock);
    renderView();
    await screen.findByText('PHI stays local');

    fireEvent.click(screen.getByRole('button', { name: /^enforce$/i }));

    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((c) => c[0]);
      expect(urls).toContain('http://aog/v1/policy/hipaa-phi-local/mode');
    });
  });
});
