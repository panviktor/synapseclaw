import { useState, useEffect } from 'react';
import {
  Cpu,
  Clock,
  Globe,
  Database,
  Activity,
  DollarSign,
  Radio,
} from 'lucide-react';
import type { StatusResponse, CostSummary } from '@/types/api';
import { getStatus, getCost } from '@/lib/api';
import { t } from '@/lib/i18n';

function formatUptime(seconds: number): string {
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (d > 0) return `${d}d ${h}h ${m}m`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

function formatUSD(value: number): string {
  return `$${value.toFixed(4)}`;
}

function healthColor(status: string): string {
  switch (status.toLowerCase()) {
    case 'ok':
    case 'healthy':
      return 'bg-[var(--status-success)]';
    case 'warn':
    case 'warning':
    case 'degraded':
      return 'bg-[#C9872C]';
    default:
      return 'bg-[var(--status-error)]';
  }
}

function healthBorder(status: string): string {
  switch (status.toLowerCase()) {
    case 'ok':
    case 'healthy':
      return 'border-[#2D8A4E]/30';
    case 'warn':
    case 'warning':
    case 'degraded':
      return 'border-[#C9872C]/30';
    default:
      return 'border-[#C73E3E]/30';
  }
}

export default function Dashboard() {
  const [status, setStatus] = useState<StatusResponse | null>(null);
  const [cost, setCost] = useState<CostSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([getStatus(), getCost()])
      .then(([s, c]) => {
        setStatus(s);
        setCost(c);
      })
      .catch((err) => setError(err.message));
  }, []);

  if (error) {
    return (
      <div className="p-6 animate-fade-in">
        <div className="rounded-xl bg-[var(--status-error)]/10 border border-[#C73E3E]/20 p-4 text-[#C73E3E]">
          {t('dashboard.load_error')}: {error}
        </div>
      </div>
    );
  }

  if (!status || !cost) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="h-8 w-8 border-2 border-[var(--accent-primary)]/20 border-t-[#D95A1E] rounded-full animate-spin" />
      </div>
    );
  }

  const maxCost = Math.max(cost.session_cost_usd, cost.daily_cost_usd, cost.monthly_cost_usd, 0.001);

  return (
    <div className="p-6 space-y-6 animate-fade-in">
      <div>
        <h1 className="text-2xl font-bold text-gradient">{t('dashboard.title')}</h1>
        <p className="text-xs text-[var(--text-muted)] mt-1">{t('dashboard.subtitle')}</p>
      </div>
      {/* Status Cards Grid */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4 stagger-children">
        {[
          { icon: Cpu, color: 'var(--accent-primary)', bg: 'var(--glow-secondary)', label: t('dashboard.provider_model'), value: status.provider ?? 'Unknown', sub: status.model },
          { icon: Clock, color: 'var(--status-success)', bg: 'var(--glow-secondary)', label: t('dashboard.uptime'), value: formatUptime(status.uptime_seconds), sub: t('dashboard.since_last_restart') },
          { icon: Globe, color: 'var(--text-muted)', bg: 'var(--bg-secondary)', label: t('dashboard.gateway_port'), value: `:${status.gateway_port}`, sub: '' },
          { icon: Database, color: 'var(--status-warning)', bg: 'var(--glow-secondary)', label: t('dashboard.memory_backend'), value: status.memory_backend, sub: `${t('dashboard.paired')}: ${status.paired ? t('dashboard.paired_yes') : t('dashboard.paired_no')}` },
        ].map(({ icon: Icon, color, bg, label, value, sub }) => (
          <div key={label} className="glass-card p-5 animate-slide-in-up">
            <div className="flex items-center gap-3 mb-3">
              <div className="p-2 rounded-xl" style={{ background: bg }}>
                <Icon className="h-5 w-5" style={{ color }} />
              </div>
              <span className="text-xs text-[var(--text-muted)] uppercase tracking-wider font-medium">{label}</span>
            </div>
            <p className="text-lg font-semibold text-[var(--text-primary)] truncate capitalize">{value}</p>
            <p className="text-sm text-[var(--text-muted)] truncate">{sub}</p>
          </div>
        ))}
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6 stagger-children">
        {/* Cost Widget */}
        <div className="glass-card p-5 animate-slide-in-up">
          <div className="flex items-center gap-2 mb-5">
            <DollarSign className="h-5 w-5 text-[var(--accent-primary)]" />
            <h2 className="text-sm font-semibold text-[var(--text-primary)] uppercase tracking-wider">{t('dashboard.cost_overview')}</h2>
          </div>
          <div className="space-y-4">
            {[
              { label: t('dashboard.session_label'), value: cost.session_cost_usd, color: 'var(--accent-primary)' },
              { label: t('dashboard.daily_label'), value: cost.daily_cost_usd, color: 'var(--status-success)' },
              { label: t('dashboard.monthly_label'), value: cost.monthly_cost_usd, color: 'var(--text-muted)' },
            ].map(({ label, value, color }) => (
              <div key={label}>
                <div className="flex justify-between text-sm mb-1.5">
                  <span className="text-[var(--text-muted)]">{label}</span>
                  <span className="text-[var(--text-primary)] font-medium font-mono">{formatUSD(value)}</span>
                </div>
                <div className="w-full h-1.5 bg-[var(--bg-hover)] rounded-full overflow-hidden">
                  <div
                    className="h-full rounded-full progress-bar-animated transition-all duration-700 ease-out"
                    style={{ width: `${Math.max((value / maxCost) * 100, 2)}%`, background: color }}
                  />
                </div>
              </div>
            ))}
          </div>
          <div className="mt-5 pt-4 border-t border-[var(--border-default)] flex justify-between text-sm">
            <span className="text-[var(--text-muted)]">{t('dashboard.total_tokens_label')}</span>
            <span className="text-[var(--text-primary)] font-mono">{cost.total_tokens.toLocaleString()}</span>
          </div>
          <div className="flex justify-between text-sm mt-1">
            <span className="text-[var(--text-muted)]">{t('dashboard.requests_label')}</span>
            <span className="text-[var(--text-primary)] font-mono">{cost.request_count.toLocaleString()}</span>
          </div>
        </div>

        {/* Active Channels */}
        <div className="glass-card p-5 animate-slide-in-up">
          <div className="flex items-center gap-2 mb-5">
            <Radio className="h-5 w-5 text-[var(--accent-primary)]" />
            <h2 className="text-sm font-semibold text-[var(--text-primary)] uppercase tracking-wider">{t('dashboard.active_channels')}</h2>
          </div>
          <div className="space-y-2">
            {Object.entries(status.channels).length === 0 ? (
              <p className="text-sm text-[var(--text-placeholder)]">{t('dashboard.no_channels')}</p>
            ) : (
              Object.entries(status.channels).map(([name, active]) => (
                <div
                  key={name}
                  className="flex items-center justify-between py-2.5 px-3 rounded-xl transition-all duration-300 hover:bg-[var(--bg-hover)]"
                >
                  <span className="text-sm text-[var(--text-primary)] capitalize font-medium">{name}</span>
                  <div className="flex items-center gap-2">
                    <span
                      className={`inline-block h-2 w-2 rounded-full ${
                        active ? 'bg-[var(--status-success)]' : 'bg-[var(--text-placeholder)]'
                      }`}
                    />
                    <span className="text-xs text-[var(--text-muted)]">
                      {active ? t('dashboard.active') : t('dashboard.inactive')}
                    </span>
                  </div>
                </div>
              ))
            )}
          </div>
        </div>

        {/* Health Grid */}
        <div className="glass-card p-5 animate-slide-in-up">
          <div className="flex items-center gap-2 mb-5">
            <Activity className="h-5 w-5 text-[var(--accent-primary)]" />
            <h2 className="text-sm font-semibold text-[var(--text-primary)] uppercase tracking-wider">{t('dashboard.component_health')}</h2>
          </div>
          <div className="grid grid-cols-2 gap-3">
            {Object.entries(status.health.components).length === 0 ? (
              <p className="text-sm text-[var(--text-placeholder)] col-span-2">{t('dashboard.no_components')}</p>
            ) : (
              Object.entries(status.health.components).map(([name, comp]) => (
                <div
                  key={name}
                  className={`rounded-xl p-3 border ${healthBorder(comp.status)} transition-all duration-300 hover:scale-[1.02]`}
                >
                  <div className="flex items-center gap-2 mb-1">
                    <span className={`inline-block h-2 w-2 rounded-full ${healthColor(comp.status)}`} />
                    <span className="text-sm font-medium text-[var(--text-primary)] capitalize truncate">
                      {name}
                    </span>
                  </div>
                  <p className="text-xs text-[var(--text-muted)] capitalize">{comp.status}</p>
                  {comp.restart_count > 0 && (
                    <p className="text-xs text-[#C9872C] mt-1">
                      {t('dashboard.restarts')}: {comp.restart_count}
                    </p>
                  )}
                </div>
              ))
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
