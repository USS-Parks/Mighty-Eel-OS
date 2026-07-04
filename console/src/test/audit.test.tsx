import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { AuditView } from '../views/AuditView';
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
  token: { token_id: 't', tenant_id: 'acme' },
  verifiedAt: '2026-07-03T00:00:00Z',
};

// A bridge issuance + a seal + an unseal, all correlated by token_id=tok_1 — the
// joined rows the unified ledger returns.
const ENTRIES = [
  {
    seq: 0,
    source: 'wsf-bridge',
    receipt: { token_id: 'tok_1', op: 'issue', subject_hash: 'h', at: '2026-07-03T00:00:00Z' },
    previous_hash: '00',
    entry_hash: 'a1',
  },
  {
    seq: 1,
    source: 'wsf-seal',
    receipt: {
      token_id: 'tok_1',
      envelope_id: 'env_9',
      op: 'seal',
      decision: 'allow',
      at: '2026-07-03T00:01:00Z',
    },
    previous_hash: 'a1',
    entry_hash: 'b2',
  },
  {
    seq: 2,
    source: 'wsf-seal',
    receipt: {
      token_id: 'tok_1',
      envelope_id: 'env_9',
      op: 'unseal',
      decision: 'allow',
      at: '2026-07-03T00:02:00Z',
    },
    previous_hash: 'b2',
    entry_hash: 'c3',
  },
];

function renderView() {
  return render(
    <AuthProvider>
      <AuditView />
    </AuthProvider>,
  );
}

beforeEach(() => {
  localStorage.clear();
  localStorage.setItem('wsf.console.session', JSON.stringify(SESSION));
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('AuditView', () => {
  it('lists the joined WSF receipts on load', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string) =>
        Promise.resolve(url.includes('/v1/receipts') ? ok({ entries: ENTRIES }) : ok({})),
      ),
    );
    renderView();

    // Three rows, all correlated by tok_1 (bridge issue + seal + unseal).
    expect(await screen.findAllByText('tok_1')).toHaveLength(3);
    expect(screen.getByText('wsf-bridge')).toBeInTheDocument();
    expect(screen.getAllByText('wsf-seal')).toHaveLength(2);
    expect(screen.getByText('3 rows')).toBeInTheDocument();
    // Envelope + op/decision surfaced.
    expect(screen.getAllByText('env_9')).toHaveLength(2);
    expect(screen.getByText('seal · allow')).toBeInTheDocument();
  });

  it('queries by correlation field + value', async () => {
    const fetchMock = vi.fn((_url: string) => Promise.resolve(ok({ entries: ENTRIES })));
    vi.stubGlobal('fetch', fetchMock);
    renderView();
    await screen.findByText('3 rows'); // initial unfiltered load complete

    fireEvent.change(screen.getByLabelText(/value/i), { target: { value: 'tok_1' } });
    const form = screen.getByRole('button', { name: /search/i }).closest('form');
    if (!form) throw new Error('search form not found');
    fireEvent.submit(form);

    const urls = fetchMock.mock.calls.map((c) => c[0]);
    expect(urls).toContain('http://wsf/v1/receipts');
    expect(urls).toContain('http://wsf/v1/receipts?field=token_id&value=tok_1');
  });
});
