import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, expect, test, vi } from 'vitest';
import App from './App';
import { useAuth } from './hooks/useAuth';

vi.mock('./hooks/useAuth');
// `useLiveEvents` opens a real WebSocket in production; stub it out here so
// this router/guard test doesn't depend on jsdom's WS support.
vi.mock('./hooks/useLiveEvents', () => ({ useLiveEvents: vi.fn() }));

const mockedUseAuth = vi.mocked(useAuth);

afterEach(() => {
  vi.clearAllMocks();
});

function renderApp(initialPath = '/') {
  return render(
    <MemoryRouter initialEntries={[initialPath]}>
      <App />
    </MemoryRouter>,
  );
}

test('shows a loading splash while auth state is unknown', () => {
  mockedUseAuth.mockReturnValue({ needsSetup: false, authed: false, loading: true, refetch: vi.fn() });
  renderApp();
  expect(screen.getByText(/loading/i)).toBeInTheDocument();
});

test('routes to Setup when needs_setup is true, regardless of path', () => {
  mockedUseAuth.mockReturnValue({ needsSetup: true, authed: false, loading: false, refetch: vi.fn() });
  renderApp('/dashboard');
  expect(screen.getByRole('button', { name: /finish setup/i })).toBeInTheDocument();
});

test('routes to Login when setup is done but not authed', () => {
  mockedUseAuth.mockReturnValue({ needsSetup: false, authed: false, loading: false, refetch: vi.fn() });
  renderApp();
  expect(screen.getByRole('button', { name: /sign in/i })).toBeInTheDocument();
});

test('renders the AppShell with full nav once authed', () => {
  mockedUseAuth.mockReturnValue({ needsSetup: false, authed: true, loading: false, refetch: vi.fn() });
  renderApp();
  expect(screen.getByRole('link', { name: /data sources/i })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: /emitters/i })).toBeInTheDocument();
  // "/" redirects to "/dashboard", which is still a stub in this task.
  expect(screen.getByRole('heading', { name: /dashboard/i })).toBeInTheDocument();
});
