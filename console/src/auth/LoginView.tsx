import { useState, type FormEvent } from 'react';
import { WsfClient } from '../api/client';
import type { TrustToken } from '../api/types';
import { useAuth } from './AuthContext';

const DEFAULT_WSF = import.meta.env.VITE_WSF_API_BASE ?? 'http://localhost:8081';
const DEFAULT_AOG = import.meta.env.VITE_AOG_BASE ?? 'http://localhost:8080';

/** WSF-identity sign-in: paste a trust token, verify it against the trust plane,
 *  and establish the session. No password store — the token is the credential. */
export function LoginView() {
  const { signIn } = useAuth();
  const [wsfBase, setWsfBase] = useState(DEFAULT_WSF);
  const [aogBase, setAogBase] = useState(DEFAULT_AOG);
  const [tokenText, setTokenText] = useState('');
  const [aogKey, setAogKey] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function onSubmit(event: FormEvent) {
    event.preventDefault();
    setError(null);

    let token: TrustToken;
    try {
      token = JSON.parse(tokenText) as TrustToken;
    } catch {
      setError('Trust token is not valid JSON.');
      return;
    }

    setBusy(true);
    try {
      const verdict = await new WsfClient(wsfBase).verifyToken(token);
      if (!verdict.valid) {
        setError(`Token rejected: ${verdict.reason}`);
        return;
      }
      signIn({
        wsfBase,
        aogBase,
        token,
        aogKey: aogKey.trim() || undefined,
        verifiedAt: new Date().toISOString(),
      });
    } catch (err) {
      setError(
        err instanceof Error
          ? `Cannot reach the WSF API: ${err.message}`
          : 'Verification failed.',
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex min-h-full items-center justify-center p-6">
      <div className="panel w-full max-w-md">
        <div className="flex items-center gap-3 border-b border-edge px-6 py-5">
          <svg viewBox="0 0 32 32" className="h-8 w-8 text-signal" aria-hidden="true">
            <path
              d="M5 25 L16 6 L27 25 Z"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.2"
              strokeLinejoin="round"
            />
            <circle cx="16" cy="19.5" r="2.4" fill="currentColor" />
          </svg>
          <div>
            <h1 className="text-base font-semibold text-ink">Sovereignty Console</h1>
            <p className="text-xs text-muted">Present a WSF trust token to continue</p>
          </div>
        </div>

        <form className="flex flex-col gap-4 px-6 py-6" onSubmit={onSubmit}>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="wsf-base" className="label-dim">
              WSF API
            </label>
            <input
              id="wsf-base"
              className="field font-mono"
              value={wsfBase}
              onChange={(e) => setWsfBase(e.target.value)}
              placeholder="http://localhost:8081"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label htmlFor="aog-base" className="label-dim">
              AOG gateway
            </label>
            <input
              id="aog-base"
              className="field font-mono"
              value={aogBase}
              onChange={(e) => setAogBase(e.target.value)}
              placeholder="http://localhost:8080"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label htmlFor="trust-token" className="label-dim">
              Trust token (JSON)
            </label>
            <textarea
              id="trust-token"
              className="field h-28 resize-y font-mono text-xs"
              value={tokenText}
              onChange={(e) => setTokenText(e.target.value)}
              placeholder='{ "token_id": "…", "tenant_id": "…", … }'
              spellCheck={false}
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label htmlFor="aog-key" className="label-dim">
              AOG virtual key{' '}
              <span className="normal-case text-muted/70">(optional)</span>
            </label>
            <input
              id="aog-key"
              className="field font-mono"
              value={aogKey}
              onChange={(e) => setAogKey(e.target.value)}
              placeholder="vk_…"
            />
          </div>

          {error ? (
            <div
              role="alert"
              className="rounded-lg border border-bad/30 bg-bad/10 px-3 py-2 text-sm text-bad"
            >
              {error}
            </div>
          ) : null}

          <button type="submit" className="btn btn-signal" disabled={busy}>
            {busy ? 'Verifying…' : 'Verify & enter'}
          </button>
        </form>
      </div>
    </div>
  );
}
