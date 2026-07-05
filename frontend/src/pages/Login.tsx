import { useState } from 'react';
import type { FormEvent } from 'react';
import { api, ApiError } from '../api/client';

/** Client-side guard only — the backend's own ceiling
 * (`auth_routes::MAX_PASSWORD_BYTES`) is the source of truth; this just
 * avoids submitting an obviously-wrong request. */
const MAX_PASSWORD_LENGTH = 1024;

export interface LoginProps {
  /** Called after a successful `POST /api/login` so the caller (`App.tsx`)
   * can re-run `useAuth`'s probe and move past the login screen. */
  onLoginSuccess: () => void | Promise<void>;
}

export default function Login({ onLoginSuccess }: LoginProps) {
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    setError(null);

    if (password.length === 0 || password.length > MAX_PASSWORD_LENGTH) {
      setError('Enter your password.');
      return;
    }

    setSubmitting(true);
    try {
      await api.login(password);
      await onLoginSuccess();
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        setError('Incorrect password.');
      } else if (err instanceof ApiError && err.status === 429) {
        setError('Too many attempts. Try again shortly.');
      } else {
        setError('Login failed. Try again.');
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="flex h-screen items-center justify-center bg-slate-950 px-4">
      <form
        onSubmit={(event) => {
          void handleSubmit(event);
        }}
        className="w-full max-w-sm space-y-4 rounded-lg border border-slate-800 bg-slate-900 p-8 shadow-xl"
      >
        <div>
          <h1 className="font-mono text-lg font-semibold text-amber-400">FluxFang</h1>
          <p className="mt-1 text-sm text-slate-400">Sign in to continue.</p>
        </div>

        <div className="space-y-1">
          <label htmlFor="login-password" className="block text-xs font-medium uppercase tracking-wide text-slate-500">
            Password
          </label>
          <input
            id="login-password"
            type="password"
            autoFocus
            autoComplete="current-password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            className="w-full rounded border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 focus:border-amber-500 focus:outline-none"
          />
        </div>

        {error && (
          <p role="alert" className="text-sm text-red-400">
            {error}
          </p>
        )}

        <button
          type="submit"
          disabled={submitting}
          className="w-full rounded bg-amber-500 px-3 py-2 text-sm font-semibold text-slate-950 transition hover:bg-amber-400 disabled:cursor-not-allowed disabled:opacity-50"
        >
          {submitting ? 'Signing in…' : 'Sign in'}
        </button>
      </form>
    </div>
  );
}
