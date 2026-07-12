// The authenticated layout: left nav to every page (real + stub) + a header
// with the live unread-notifications badge and Logout. Page content renders
// via `<Outlet/>`, wired up by `App.tsx`'s nested routes.
import { NavLink, Outlet } from 'react-router-dom';
import { useUnreadCount } from '../store/notificationStore';

export interface AppShellProps {
  onLogout: () => void | Promise<void>;
}

const NAV_ITEMS: ReadonlyArray<{ to: string; label: string }> = [
  { to: '/dashboard', label: 'Dashboard' },
  { to: '/data-sources', label: 'Data Sources' },
  { to: '/emissions', label: 'Emissions' },
  { to: '/emitters', label: 'Emitters' },
  { to: '/co-travel', label: 'Co-Travel' },
  { to: '/entities', label: 'Entities' },
  { to: '/zones', label: 'Zones' },
  { to: '/map', label: 'Map' },
  { to: '/alerts', label: 'Alerts' },
  { to: '/notifications', label: 'Notifications' },
];

function navLinkClassName({ isActive }: { isActive: boolean }): string {
  return `block rounded px-3 py-2 text-sm transition ${
    isActive ? 'bg-amber-500/10 text-amber-400' : 'text-slate-400 hover:bg-slate-800/60 hover:text-slate-100'
  }`;
}

export default function AppShell({ onLogout }: AppShellProps) {
  const unread = useUnreadCount();

  return (
    <div className="flex h-screen bg-slate-950 text-slate-200">
      <aside className="flex w-56 flex-shrink-0 flex-col border-r border-slate-800 bg-slate-900/60">
        <div className="px-4 py-4 font-mono text-sm font-semibold tracking-wide text-amber-400">FluxFang</div>
        <nav className="flex-1 space-y-0.5 px-2">
          {NAV_ITEMS.map((item) => (
            <NavLink key={item.to} to={item.to} className={navLinkClassName}>
              {item.label}
            </NavLink>
          ))}
        </nav>
      </aside>

      <div className="flex flex-1 flex-col overflow-hidden">
        <header className="flex items-center justify-between border-b border-slate-800 px-6 py-3">
          <NavLink to="/notifications" className="flex items-center gap-2 text-sm text-slate-400 hover:text-slate-100">
            Notifications
            {unread > 0 && (
              <span
                data-testid="unread-badge"
                className="inline-flex h-5 min-w-5 items-center justify-center rounded-full bg-amber-500 px-1 text-xs font-semibold text-slate-950"
              >
                {unread}
              </span>
            )}
          </NavLink>

          <button
            type="button"
            onClick={() => {
              void onLogout();
            }}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 transition hover:border-slate-500 hover:text-slate-100"
          >
            Logout
          </button>
        </header>

        <main className="flex-1 overflow-auto p-6">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
