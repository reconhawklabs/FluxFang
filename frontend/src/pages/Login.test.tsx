import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import Login from './Login';

afterEach(() => {
  vi.unstubAllGlobals();
});

test('posts the password and calls onLoginSuccess on success', async () => {
  const fetchMock = vi.fn().mockResolvedValue({
    ok: true,
    status: 200,
    statusText: 'OK',
    text: async () => '',
    clone() {
      return this;
    },
  });
  vi.stubGlobal('fetch', fetchMock);

  const onLoginSuccess = vi.fn().mockResolvedValue(undefined);
  render(<Login onLoginSuccess={onLoginSuccess} />);

  fireEvent.change(screen.getByLabelText(/password/i), { target: { value: 'correct-horse' } });
  fireEvent.click(screen.getByRole('button', { name: /sign in/i }));

  await waitFor(() => expect(onLoginSuccess).toHaveBeenCalledTimes(1));

  expect(fetchMock).toHaveBeenCalledTimes(1);
  const [url, init] = fetchMock.mock.calls[0];
  expect(url).toBe('/api/login');
  expect(init).toMatchObject({ method: 'POST', credentials: 'include' });
  expect(JSON.parse(init.body as string)).toEqual({ password: 'correct-horse' });
});

test('shows an error and does not call onLoginSuccess on a 401', async () => {
  const fetchMock = vi.fn().mockResolvedValue({
    ok: false,
    status: 401,
    statusText: 'Unauthorized',
    text: async () => '',
    clone() {
      return this;
    },
    json: async () => {
      throw new Error('not json');
    },
  });
  vi.stubGlobal('fetch', fetchMock);

  const onLoginSuccess = vi.fn();
  render(<Login onLoginSuccess={onLoginSuccess} />);

  fireEvent.change(screen.getByLabelText(/password/i), { target: { value: 'wrong' } });
  fireEvent.click(screen.getByRole('button', { name: /sign in/i }));

  await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent(/incorrect password/i));
  expect(onLoginSuccess).not.toHaveBeenCalled();
});
