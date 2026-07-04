import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { BrowserRouter } from 'react-router-dom';
import App from '../App';
import { AuthProvider } from '../auth/AuthContext';

function renderApp() {
  return render(
    <BrowserRouter>
      <AuthProvider>
        <App />
      </AuthProvider>
    </BrowserRouter>,
  );
}

function ok(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as unknown as Response;
}

const TOKEN = JSON.stringify({ token_id: 'tok_demo_1234567890', tenant_id: 'acme' });

function submitLogin() {
  fireEvent.change(screen.getByLabelText(/trust token/i), { target: { value: TOKEN } });
  const button = screen.getByRole('button', { name: /verify & enter/i });
  const form = button.closest('form');
  if (!form) throw new Error('login form not found');
  fireEvent.submit(form);
}

beforeEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('login', () => {
  it('round-trips a valid token into the authed shell', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((_url: string, _init?: RequestInit) =>
        Promise.resolve(ok({ valid: true, reason: 'ok' })),
      ),
    );
    renderApp();

    // The login screen is shown first (no session).
    expect(screen.getByRole('button', { name: /verify & enter/i })).toBeInTheDocument();

    submitLogin();

    // The shell renders — "Sign out" is unique to the authed frame.
    expect(await screen.findByRole('button', { name: /sign out/i })).toBeInTheDocument();
    // The tenant from the token surfaces (top bar + overview panel).
    expect(screen.getAllByText('acme').length).toBeGreaterThan(0);
  });

  it('surfaces the rejection reason for an invalid token and stays on login', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((_url: string, _init?: RequestInit) =>
        Promise.resolve(ok({ valid: false, reason: 'bad signature' })),
      ),
    );
    renderApp();

    submitLogin();

    expect(await screen.findByRole('alert')).toHaveTextContent(/bad signature/i);
    expect(screen.queryByRole('button', { name: /sign out/i })).not.toBeInTheDocument();
  });

  it('rejects malformed JSON before calling the API', async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(ok({ valid: true, reason: 'ok' })),
    );
    vi.stubGlobal('fetch', fetchMock);
    renderApp();

    fireEvent.change(screen.getByLabelText(/trust token/i), {
      target: { value: 'not json' },
    });
    const button = screen.getByRole('button', { name: /verify & enter/i });
    const form = button.closest('form');
    if (!form) throw new Error('login form not found');
    fireEvent.submit(form);

    expect(await screen.findByRole('alert')).toHaveTextContent(/not valid json/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
