import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { SessionReplayView } from '../views/SessionReplayView';
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

const SESSIONS = [{ session_id: 'sess-1', steps: 3, chain_verified: true }];

const DETAIL = {
  session_id: 'sess-1',
  chain_verified: true,
  steps: [
    { seq: 0, kind: 'prompt', at: 't0', actor: 'user', summary: 'summarise the incident' },
    { seq: 1, kind: 'tool_call', at: 't1', actor: 'read.log', summary: 'read.log call c1' },
    { seq: 2, kind: 'tool_result', at: 't2', actor: 'read.log', summary: 'read.log ok' },
  ],
};

function routed(url: string): Response {
  if (/\/v1\/sessions\/[^/?]+/.test(url)) return ok(DETAIL);
  if (url.includes('/v1/sessions')) return ok({ sessions: SESSIONS });
  return ok({});
}

function renderView() {
  return render(
    <AuthProvider>
      <SessionReplayView />
    </AuthProvider>,
  );
}

beforeEach(() => {
  localStorage.clear();
  localStorage.setItem('wsf.console.session', JSON.stringify(SESSION));
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('SessionReplayView', () => {
  it('loads a recorded session and shows the first step, verified', async () => {
    vi.stubGlobal('fetch', vi.fn((url: string) => Promise.resolve(routed(url))));
    renderView();

    expect(await screen.findByText('Step 1 of 3')).toBeInTheDocument();
    expect(screen.getByText('Verified')).toBeInTheDocument();
    // The first step (the prompt) is the current step.
    expect(screen.getAllByText('summarise the incident').length).toBeGreaterThan(0);
  });

  it('plays back step-by-step (the gate)', async () => {
    vi.stubGlobal('fetch', vi.fn((url: string) => Promise.resolve(routed(url))));
    renderView();
    await screen.findByText('Step 1 of 3');

    const next = screen.getByRole('button', { name: /next/i });
    fireEvent.click(next);
    expect(await screen.findByText('Step 2 of 3')).toBeInTheDocument();
    // The tool_call step is now current.
    expect(screen.getAllByText('read.log call c1').length).toBeGreaterThan(0);

    fireEvent.click(next);
    expect(await screen.findByText('Step 3 of 3')).toBeInTheDocument();
  });
});
