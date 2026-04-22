import { NavLink } from 'react-router-dom';
import { useState, useEffect } from 'react';
import {
  LayoutDashboard,
  MessageSquare,
  Wrench,
  Clock,
  Puzzle,
  Brain,
  BookMarked,
  Settings,
  DollarSign,
  Activity,
  Stethoscope,
  Network,
  Users,
  ScrollText,
  Rocket,
  ShieldAlert,
  FileSearch,
  Radio,
  Timer,
  AlertTriangle,
  GitBranch,
  X,
} from 'lucide-react';
import { t } from '@/lib/i18n';
import { checkIpcAccess, fetchMessages } from '@/lib/ipc-api';

interface NavItem {
  to: string;
  icon: React.ComponentType<{ className?: string }>;
  labelKey: string;
  descKey?: string;
  end?: boolean;
}

const navItems: NavItem[] = [
  { to: '/', icon: LayoutDashboard, labelKey: 'nav.dashboard', descKey: 'dashboard.subtitle', end: true },
  { to: '/agents', icon: MessageSquare, labelKey: 'nav.agents', descKey: 'agent.subtitle' },
  { to: '/tools', icon: Wrench, labelKey: 'nav.tools', descKey: 'tools.subtitle' },
  { to: '/cron', icon: Clock, labelKey: 'nav.cron', descKey: 'cron.subtitle' },
  { to: '/integrations', icon: Puzzle, labelKey: 'nav.integrations', descKey: 'integrations.subtitle' },
  { to: '/memory', icon: Brain, labelKey: 'nav.memory', descKey: 'memory.subtitle' },
  { to: '/skills', icon: BookMarked, labelKey: 'nav.skills', descKey: 'skills.subtitle' },
  { to: '/config', icon: Settings, labelKey: 'nav.config', descKey: 'config.subtitle' },
  { to: '/cost', icon: DollarSign, labelKey: 'nav.cost', descKey: 'cost.subtitle' },
  { to: '/logs', icon: Activity, labelKey: 'nav.logs', descKey: 'logs.subtitle' },
  { to: '/doctor', icon: Stethoscope, labelKey: 'nav.doctor', descKey: 'doctor.subtitle' },
];

const ipcNavItems: NavItem[] = [
  { to: '/ipc/fleet', icon: Users, labelKey: 'nav.ipc_fleet', descKey: 'ipc.fleet_subtitle' },
  { to: '/ipc/activity', icon: Radio, labelKey: 'nav.ipc_activity', descKey: 'ipc.activity_subtitle' },
  { to: '/ipc/sessions', icon: ScrollText, labelKey: 'nav.ipc_sessions', descKey: 'ipc.sessions_subtitle' },
  { to: '/ipc/spawns', icon: Rocket, labelKey: 'nav.ipc_spawns', descKey: 'ipc.spawns_subtitle' },
  { to: '/ipc/quarantine', icon: ShieldAlert, labelKey: 'nav.ipc_quarantine', descKey: 'ipc.quarantine_subtitle' },
  { to: '/ipc/audit', icon: FileSearch, labelKey: 'nav.ipc_audit', descKey: 'ipc.audit_subtitle' },
  { to: '/ipc/cron', icon: Timer, labelKey: 'nav.ipc_cron', descKey: 'ipc.cron_subtitle' },
  { to: '/ipc/dead-letters', icon: AlertTriangle, labelKey: 'Dead Letters', descKey: 'Failed pipeline steps' },
  { to: '/ipc/pipelines', icon: GitBranch, labelKey: 'Pipelines', descKey: 'Pipeline graph visualization' },
];

function NavLinkItem({ to, icon: Icon, labelKey, descKey, end, idx, badge }: NavItem & { idx: number; badge?: number }) {
  return (
    <NavLink
      key={to}
      to={to}
      end={end}
      title={descKey ? t(descKey) : undefined}
      className={({ isActive }) =>
        [
          'flex items-center gap-3 px-3 py-2.5 rounded-lg text-sm font-medium transition-all duration-200 animate-slide-in-left group',
          isActive
            ? 'text-theme-primary'
            : 'text-theme-muted hover:text-theme-primary hover:bg-theme-hover',
        ].join(' ')
      }
      style={({ isActive }) => ({
        animationDelay: `${idx * 40}ms`,
        ...(isActive ? { background: 'var(--bg-secondary)', borderLeft: '3px solid var(--accent-primary)' } : {}),
      })}
    >
      {({ isActive }) => (
        <>
          <Icon className={`h-5 w-5 flex-shrink-0 transition-colors duration-200 ${isActive ? 'text-theme-accent' : 'group-hover:text-theme-accent'}`} />
          <span>{t(labelKey)}</span>
          {badge !== undefined && badge > 0 && (
            <span className="ml-auto px-1.5 py-0.5 rounded-full text-[10px] font-semibold bg-theme-accent/10 text-theme-accent min-w-[20px] text-center">
              {badge}
            </span>
          )}
          {isActive && !badge && (
            <div className="ml-auto h-1.5 w-1.5 rounded-full bg-theme-accent" />
          )}
        </>
      )}
    </NavLink>
  );
}

interface SidebarProps {
  isOpen: boolean;
  onClose: () => void;
}

function NavLinkItemWithClose({
  onNavigate,
  ...props
}: NavItem & { idx: number; badge?: number; onNavigate?: () => void }) {
  return (
    <div onClick={onNavigate}>
      <NavLinkItem {...props} />
    </div>
  );
}

export default function Sidebar({ isOpen, onClose }: SidebarProps) {
  const [ipcAvailable, setIpcAvailable] = useState(false);
  const [quarantineCount, setQuarantineCount] = useState(0);

  useEffect(() => {
    checkIpcAccess().then(setIpcAvailable);
  }, []);

  // Poll quarantine pending count
  useEffect(() => {
    if (!ipcAvailable) return;

    const pollQuarantine = () => {
      fetchMessages({ quarantine: true, dismissed: false, limit: 200 })
        .then((msgs) => {
          const pending = msgs.filter((m) => !m.promoted && !m.blocked).length;
          setQuarantineCount(pending);
        })
        .catch(() => {});
    };

    pollQuarantine();
    const interval = setInterval(pollQuarantine, 30_000);
    return () => clearInterval(interval);
  }, [ipcAvailable]);

  return (
    <>
      <button
        type="button"
        aria-label="Close navigation"
        className={`fixed inset-0 z-30 bg-black/30 backdrop-blur-[1px] transition-opacity duration-300 md:hidden ${
          isOpen ? 'opacity-100' : 'pointer-events-none opacity-0'
        }`}
        onClick={onClose}
      />

      <aside
        className={`fixed inset-y-0 left-0 z-40 flex h-screen w-[min(84vw,18rem)] flex-col border-r border-theme-default bg-theme-sidebar transition-transform duration-300 ease-out md:z-30 md:w-60 ${
          isOpen
            ? 'translate-x-0 shadow-2xl md:shadow-none'
            : '-translate-x-full pointer-events-none md:pointer-events-auto md:translate-x-0 md:shadow-none'
        }`}
      >
        <div className="sidebar-accent-line" />

        <div className="flex items-center justify-between gap-3 border-b border-theme-default px-4 py-4">
          <div className="flex items-center gap-3">
            <img
              src="/_app/logo.png"
              alt="SynapseClaw"
              className="h-10 w-10 rounded-xl object-cover"
            />
            <span className="text-lg font-bold text-gradient tracking-wide">
              SynapseClaw
            </span>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-xl border border-theme-default bg-theme-card p-2 text-theme-muted transition-colors hover:bg-theme-hover hover:text-theme-primary md:hidden"
            aria-label="Close navigation"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <nav className="flex-1 overflow-y-auto py-4 px-3 space-y-1">
          {navItems.map((item, idx) => (
            <NavLinkItemWithClose key={item.to} {...item} idx={idx} onNavigate={onClose} />
          ))}

          {ipcAvailable && (
            <>
              <div className="pt-4 pb-1 px-3">
                <div className="flex items-center gap-2 text-[10px] text-theme-placeholder tracking-wider uppercase font-semibold">
                  <Network className="h-3 w-3" />
                  <span>{t('nav.ipc_section')}</span>
                </div>
              </div>
              {ipcNavItems.map((item, idx) => (
                <NavLinkItemWithClose
                  key={item.to}
                  {...item}
                  idx={navItems.length + idx}
                  badge={item.to === '/ipc/quarantine' ? quarantineCount : undefined}
                  onNavigate={onClose}
                />
              ))}
            </>
          )}
        </nav>

        <div className="border-t border-theme-default px-5 py-4">
          <p className="text-[10px] text-theme-placeholder tracking-wider uppercase">SynapseClaw Runtime</p>
        </div>
      </aside>
    </>
  );
}
