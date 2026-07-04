import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { ApprovalsView } from '../views/ApprovalsView';
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

const TICKET = {
  id: 'tk_1',
  tool_id: 'pager.page',
  session_id: 's1',
  profile_id: 'tok_1',
  summary: 'page the on-call engineer',
  diff_preview: 'pager.page {"team":"sre","severity":"high"}',
  requested_at: '2026-07-04T00:00:00Z',
  status: 'pending',
};

function renderView() {
  return render(
    <AuthProvider>
      <ApprovalsView />
    </AuthProvider>,
  );
}

beforeEach(() => {
  localStorage.clear();
  localStorage.setItem('wsf.console.session', JSON.stringify(SESSION));
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('ApprovalsView', () => {
  it('lists pending approvals with a diff preview', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string) =>
        Promise.resolve(url.includes('/v1/approvals') ? ok({ pending: [TICKET] }) : ok({})),
      ),
    );
    renderView();

    expect(await screen.findByText('pager.page')).toBeInTheDocument();
    expect(screen.getByText('page the on-call engineer')).toBeInTheDocument();
    expect(screen.getByText('1 pending')).toBeInTheDocument();
    // The diff preview — what the call will actually do — is shown to the approver.
    expect(screen.getByText(/"team":"sre"/)).toBeInTheDocument();
  });

  it('an approve click drives the approve endpoint with the actor', async () => {
    const fetchMock = vi.fn((url: string) =>
      Promise.resolve(
        url.endsWith('/approve')
          ? ok({ id: 'tk_1', status: 'approved', actor: 't' })
          : ok({ pending: [TICKET] }),
      ),
    );
    vi.stubGlobal('fetch', fetchMock);
    renderView();

    fireEvent.click(await screen.findByRole('button', { name: /approve/i }));

    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((c) => c[0]);
      expect(urls).toContain('http://aog/v1/approvals/tk_1/approve');
    });
    // The recorded decision + actor surface back to the operator.
    expect(await screen.findByText(/pager\.page approved by t/)).toBeInTheDocument();
  });

  it('a deny click drives the deny endpoint', async () => {
    const fetchMock = vi.fn((url: string) =>
      Promise.resolve(
        url.endsWith('/deny')
          ? ok({ id: 'tk_1', status: 'denied', actor: 't' })
          : ok({ pending: [TICKET] }),
      ),
    );
    vi.stubGlobal('fetch', fetchMock);
    renderView();

    fireEvent.click(await screen.findByRole('button', { name: /deny/i }));

    await waitFor(() => {
      const urls = fetchMock.mock.calls.map((c) => c[0]);
      expect(urls).toContain('http://aog/v1/approvals/tk_1/deny');
    });
  });
});
