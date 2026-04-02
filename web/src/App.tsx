import { Routes, Route, Navigate } from 'react-router-dom';
import { useState, useEffect, createContext, useContext, Component } from 'react';
import type { ReactNode, ErrorInfo } from 'react';
import Layout from './components/layout/Layout';
import Dashboard from './pages/Dashboard';
import AgentChat from './pages/AgentChat';
import Tools from './pages/Tools';
import Cron from './pages/Cron';
import Integrations from './pages/Integrations';
import Memory from './pages/Memory';
import Config from './pages/Config';
import Cost from './pages/Cost';
import Logs from './pages/Logs';
import Doctor from './pages/Doctor';
import IpcFleet from './pages/ipc/Fleet';
import IpcAgentDetail from './pages/ipc/AgentDetail';
import IpcSessions from './pages/ipc/Sessions';
import IpcSpawns from './pages/ipc/Spawns';
import IpcQuarantine from './pages/ipc/Quarantine';
import IpcAudit from './pages/ipc/Audit';
import IpcActivity from './pages/ipc/Activity';
import IpcCron from './pages/ipc/Cron';
import IpcConversation from './pages/ipc/Conversation';
import IpcDeadLetters from './pages/ipc/DeadLetters';
import IpcPipelineGraph from './pages/ipc/PipelineGraph';
import { AuthProvider, useAuth } from './hooks/useAuth';
import { ThemeProvider } from './hooks/useTheme';
import { setLocale, type Locale } from './lib/i18n';

// Locale context
interface LocaleContextType {
  locale: string;
  setAppLocale: (locale: string) => void;
}

export const LocaleContext = createContext<LocaleContextType>({
  locale: 'en',
  setAppLocale: () => {},
});

export const useLocaleContext = () => useContext(LocaleContext);

// ---------------------------------------------------------------------------
// Error boundary — catches render crashes and shows a recoverable message
// instead of a black screen
// ---------------------------------------------------------------------------

interface ErrorBoundaryState {
  error: Error | null;
}

export class ErrorBoundary extends Component<
  { children: ReactNode },
  ErrorBoundaryState
> {
  constructor(props: { children: ReactNode }) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('[SynapseClaw] Render error:', error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="p-6">
          <div className="bg-theme-card border border-theme-default rounded-xl p-6 w-full max-w-lg shadow-sm">
            <h2 className="text-lg font-semibold text-status-error mb-2">
              Something went wrong
            </h2>
            <p className="text-theme-muted text-sm mb-4">
              A render error occurred. Check the browser console for details.
            </p>
            <pre className="text-xs text-status-error bg-theme-secondary rounded p-3 overflow-x-auto whitespace-pre-wrap break-all">
              {this.state.error.message}
            </pre>
            <button
              onClick={() => this.setState({ error: null })}
              className="mt-6 px-4 py-2 bg-theme-accent hover:bg-theme-accent-hover text-white text-sm font-medium rounded-lg transition-colors"
            >
              Try again
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

// Pairing dialog component
function PairingDialog({ onPair }: { onPair: (code: string) => Promise<void> }) {
  const [code, setCode] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError('');
    try {
      await onPair(code);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Pairing failed');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen flex items-center justify-center bg-theme-primary">
      <div className="relative glass-card p-8 w-full max-w-md animate-fade-in-scale">
        {/* Top accent line */}
        <div className="absolute -top-px left-1/4 right-1/4 h-px bg-gradient-to-r from-transparent via-theme-accent to-transparent" />

        <div className="text-center mb-8">
          <img
            src="/_app/logo.png"
            alt="SynapseClaw"
            className="h-20 w-20 rounded-2xl object-cover mx-auto mb-4"
            style={{ boxShadow: '0 4px 20px var(--glow-primary)' }}
          />
          <h1 className="text-2xl font-bold text-gradient mb-2">SynapseClaw</h1>
          <p className="text-theme-muted text-sm">Enter the pairing code from your terminal</p>
        </div>
        <form onSubmit={handleSubmit}>
          <input
            type="text"
            value={code}
            onChange={(e) => setCode(e.target.value)}
            placeholder="6-digit code"
            className="input-warm w-full px-4 py-4 text-center text-2xl tracking-[0.3em] font-medium mb-4"
            maxLength={6}
            autoFocus
          />
          {error && (
            <p className="text-status-error text-sm mb-4 text-center animate-fade-in" aria-live="polite">{error}</p>
          )}
          <button
            type="submit"
            disabled={loading || code.length < 6}
            className="btn-primary w-full py-3.5 text-sm font-semibold tracking-wide"
          >
            {loading ? (
              <span className="flex items-center justify-center gap-2">
                <span className="h-4 w-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                Pairing...
              </span>
            ) : 'Pair'}
          </button>
        </form>
      </div>
    </div>
  );
}

function AppContent() {
  const { isAuthenticated, requiresPairing, loading, pair, logout } = useAuth();
  const [locale, setLocaleState] = useState('en');

  const setAppLocale = (newLocale: string) => {
    setLocaleState(newLocale);
    setLocale(newLocale as Locale);
  };

  // Listen for 401 events to force logout
  useEffect(() => {
    const handler = () => {
      logout();
    };
    window.addEventListener('synapseclaw-unauthorized', handler);
    return () => window.removeEventListener('synapseclaw-unauthorized', handler);
  }, [logout]);

  if (loading) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-theme-primary">
        <div className="flex flex-col items-center gap-4 animate-fade-in">
          <div className="h-10 w-10 border-2 border-theme-accent/20 border-t-theme-accent rounded-full animate-spin" />
          <p className="text-theme-muted text-sm">Connecting...</p>
        </div>
      </div>
    );
  }

  if (!isAuthenticated && requiresPairing) {
    return <PairingDialog onPair={pair} />;
  }

  return (
    <LocaleContext.Provider value={{ locale, setAppLocale }}>
      <Routes>
        <Route element={<Layout />}>
          <Route path="/" element={<Dashboard />} />
          <Route path="/agents" element={<AgentChat />} />
          <Route path="/agent" element={<Navigate to="/agents" replace />} />
          <Route path="/tools" element={<Tools />} />
          <Route path="/cron" element={<Cron />} />
          <Route path="/integrations" element={<Integrations />} />
          <Route path="/memory" element={<Memory />} />
          <Route path="/config" element={<Config />} />
          <Route path="/cost" element={<Cost />} />
          <Route path="/logs" element={<Logs />} />
          <Route path="/doctor" element={<Doctor />} />
          {/* IPC Phase 3.5 pages */}
          <Route path="/ipc/fleet" element={<IpcFleet />} />
          <Route path="/ipc/fleet/:agentId" element={<IpcAgentDetail />} />
          <Route path="/ipc/sessions" element={<IpcSessions />} />
          <Route path="/ipc/spawns" element={<IpcSpawns />} />
          <Route path="/ipc/quarantine" element={<IpcQuarantine />} />
          <Route path="/ipc/audit" element={<IpcAudit />} />
          <Route path="/ipc/activity" element={<IpcActivity />} />
          <Route path="/ipc/cron" element={<IpcCron />} />
          <Route path="/ipc/conversation" element={<IpcConversation />} />
          {/* Phase 4.5: Pipeline hardening */}
          <Route path="/ipc/dead-letters" element={<IpcDeadLetters />} />
          <Route path="/ipc/pipelines" element={<IpcPipelineGraph />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </LocaleContext.Provider>
  );
}

export default function App() {
  return (
    <ThemeProvider>
      <AuthProvider>
        <AppContent />
      </AuthProvider>
    </ThemeProvider>
  );
}
