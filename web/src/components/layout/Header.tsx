import { useLocation } from 'react-router-dom';
import { LogOut, Sun, Moon, Monitor, PanelLeftOpen } from 'lucide-react';
import { t } from '@/lib/i18n';
import { useLocaleContext } from '@/App';
import { useAuth } from '@/hooks/useAuth';
import { useTheme } from '@/hooks/useTheme';

const routeTitles: Array<{ prefix: string; title: string }> = [
  { prefix: '/ipc/fleet', title: 'nav.ipc_fleet' },
  { prefix: '/ipc/activity', title: 'nav.ipc_activity' },
  { prefix: '/learning-patterns', title: 'Learning Patterns' },
  { prefix: '/agents', title: 'nav.agents' },
  { prefix: '/', title: 'nav.dashboard' },
  { prefix: '/agent', title: 'nav.agent' },
  { prefix: '/tools', title: 'nav.tools' },
  { prefix: '/cron', title: 'nav.cron' },
  { prefix: '/integrations', title: 'nav.integrations' },
  { prefix: '/memory', title: 'nav.memory' },
  { prefix: '/config', title: 'nav.config' },
  { prefix: '/cost', title: 'nav.cost' },
  { prefix: '/logs', title: 'nav.logs' },
  { prefix: '/doctor', title: 'nav.doctor' },
];

interface HeaderProps {
  onOpenSidebar?: () => void;
}

export default function Header({ onOpenSidebar }: HeaderProps) {
  const location = useLocation();
  const { logout } = useAuth();
  const { locale, setAppLocale } = useLocaleContext();
  const { theme, toggleTheme } = useTheme();

  const titleKey =
    routeTitles.find(({ prefix }) => location.pathname === prefix || location.pathname.startsWith(`${prefix}/`))
      ?.title ?? 'nav.dashboard';
  const pageTitle = t(titleKey);

  const toggleLanguage = () => {
    // Cycle through: en -> zh -> tr -> en
    const nextLocale = locale === 'en' ? 'zh' : locale === 'zh' ? 'tr' : 'en';
    setAppLocale(nextLocale);
  };

  const getThemeIcon = () => {
    switch (theme) {
      case 'light':
        return <Sun className="h-3.5 w-3.5" />;
      case 'dark':
        return <Moon className="h-3.5 w-3.5" />;
      case 'auto':
        return <Monitor className="h-3.5 w-3.5" />;
    }
  };

  const getThemeLabel = () => {
    switch (theme) {
      case 'light':
        return 'Light';
      case 'dark':
        return 'Dark';
      case 'auto':
        return 'Auto';
    }
  };

  return (
    <header className="sticky top-0 z-20 flex h-14 items-center justify-between gap-3 border-b border-theme-default bg-theme-card/80 px-4 backdrop-blur-sm animate-fade-in md:px-6">
      <div className="flex min-w-0 items-center gap-3">
        <button
          type="button"
          onClick={onOpenSidebar}
          className="inline-flex rounded-xl border border-theme-default bg-theme-card p-2 text-theme-muted transition-all duration-200 hover:border-theme-accent/30 hover:text-theme-primary hover:bg-theme-hover md:hidden"
          aria-label="Open navigation"
        >
          <PanelLeftOpen className="h-4 w-4" />
        </button>
        <div className="min-w-0">
          <h1 className="truncate text-base font-semibold tracking-tight text-theme-primary md:text-lg">{pageTitle}</h1>
          <p className="hidden text-[11px] uppercase tracking-[0.2em] text-theme-placeholder sm:block">
            SynapseClaw control surface
          </p>
        </div>
      </div>

      <div className="flex items-center gap-2 sm:gap-3">
        <button
          type="button"
          onClick={toggleTheme}
          className="flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs text-theme-muted transition-all duration-200 hover:bg-theme-hover hover:text-theme-primary sm:px-3"
          title={`Theme: ${getThemeLabel()}`}
        >
          {getThemeIcon()}
          <span className="hidden sm:inline">{getThemeLabel()}</span>
        </button>

        <button
          type="button"
          onClick={toggleLanguage}
          className="rounded-lg border border-theme-default px-2.5 py-1 text-xs font-semibold text-theme-muted transition-all duration-200 hover:border-theme-accent/30 hover:bg-theme-accent/5 hover:text-theme-primary sm:px-3"
        >
          {locale === 'en' ? 'EN' : 'TR'}
        </button>

        <button
          type="button"
          onClick={logout}
          className="flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs text-theme-muted transition-all duration-200 hover:bg-status-error/5 hover:text-status-error sm:px-3"
        >
          <LogOut className="h-3.5 w-3.5" />
          <span className="hidden sm:inline">{t('auth.logout')}</span>
        </button>
      </div>
    </header>
  );
}
