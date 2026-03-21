import { useLocation } from 'react-router-dom';
import { LogOut, Sun, Moon, Monitor } from 'lucide-react';
import { t } from '@/lib/i18n';
import { useLocaleContext } from '@/App';
import { useAuth } from '@/hooks/useAuth';
import { useTheme } from '@/hooks/useTheme';

const routeTitles: Record<string, string> = {
  '/': 'nav.dashboard',
  '/agent': 'nav.agent',
  '/tools': 'nav.tools',
  '/cron': 'nav.cron',
  '/integrations': 'nav.integrations',
  '/memory': 'nav.memory',
  '/config': 'nav.config',
  '/cost': 'nav.cost',
  '/logs': 'nav.logs',
  '/doctor': 'nav.doctor',
};

export default function Header() {
  const location = useLocation();
  const { logout } = useAuth();
  const { locale, setAppLocale } = useLocaleContext();
  const { theme, toggleTheme } = useTheme();

  const titleKey = routeTitles[location.pathname] ?? 'nav.dashboard';
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
    <header className="h-14 flex items-center justify-between px-6 border-b border-theme-default animate-fade-in bg-theme-card/80 backdrop-blur-sm">
      {/* Page title */}
      <h1 className="text-lg font-semibold text-theme-primary tracking-tight">{pageTitle}</h1>

      {/* Right-side controls */}
      <div className="flex items-center gap-3">
        {/* Theme toggle */}
        <button
          type="button"
          onClick={toggleTheme}
          className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs text-theme-muted hover:text-theme-primary hover:bg-theme-hover transition-all duration-200"
          title={`Theme: ${getThemeLabel()}`}
        >
          {getThemeIcon()}
          <span>{getThemeLabel()}</span>
        </button>

        {/* Language switcher */}
        <button
          type="button"
          onClick={toggleLanguage}
          className="px-3 py-1 rounded-lg text-xs font-semibold border border-theme-default text-theme-muted hover:text-theme-primary hover:border-theme-accent/30 hover:bg-theme-accent/5 transition-all duration-200"
        >
          {locale === 'en' ? 'EN' : 'TR'}
        </button>

        {/* Logout */}
        <button
          type="button"
          onClick={logout}
          className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs text-theme-muted hover:text-status-error hover:bg-status-error/5 transition-all duration-200"
        >
          <LogOut className="h-3.5 w-3.5" />
          <span>{t('auth.logout')}</span>
        </button>
      </div>
    </header>
  );
}
