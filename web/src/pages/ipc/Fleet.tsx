import { useState, useEffect, useCallback, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { t } from '@/lib/i18n';
import { fetchTopology, deleteAgent, revokeAgent, quarantineAgent, disableAgent, downgradeAgent } from '@/lib/ipc-api';
import type { TopologyAgent, TopologyEdge } from '@/lib/ipc-api';
import { getStatus } from '@/lib/api';
import TrustBadge from '@/components/ipc/TrustBadge';
import StatusBadge from '@/components/ipc/StatusBadge';
import TimeAgo from '@/components/ipc/TimeAgo';
import ConfirmDialog from '@/components/ipc/ConfirmDialog';
import AddAgentDialog from '@/components/ipc/AddAgentDialog';
import DeployBlueprintDialog from '@/components/ipc/DeployBlueprintDialog';

type ActionType = 'revoke' | 'quarantine' | 'disable' | 'downgrade' | 'delete';

interface PendingAction {
  type: ActionType;
  agent: TopologyAgent;
  level?: number;
}

// ── Trust-level colors ──────────────────────────────────────
const TRUST_COLORS: Record<number, string> = {
  0: '#00ff88',
  1: '#00ccff',
  2: '#0080ff',
  3: '#8892a8',
  4: '#ff6644',
};
function trustColor(level: number | null): string {
  return TRUST_COLORS[level ?? 3] ?? '#8892a8';
}

// ── Circle layout for N nodes ───────────────────────────────
function circleLayout(count: number, cx: number, cy: number, r: number) {
  return Array.from({ length: count }, (_, i) => {
    const angle = (2 * Math.PI * i) / count - Math.PI / 2;
    return { x: cx + r * Math.cos(angle), y: cy + r * Math.sin(angle) };
  });
}

// ── SVG Topology Graph ──────────────────────────────────────
function TopologyGraph({
  agents,
  edges,
  onSelect,
}: {
  agents: TopologyAgent[];
  edges: TopologyEdge[];
  onSelect: (agentId: string) => void;
}) {
  const width = 700;
  const height = 400;
  const cx = width / 2;
  const cy = height / 2;
  const radius = Math.min(cx, cy) - 60;
  const nodeRadius = 24;

  const positions = useMemo(
    () => circleLayout(agents.length, cx, cy, agents.length === 1 ? 0 : radius),
    [agents.length, cx, cy, radius],
  );

  const agentIdx = useMemo(() => {
    const map = new Map<string, number>();
    agents.forEach((a, i) => map.set(a.agent_id, i));
    return map;
  }, [agents]);

  const [hovered, setHovered] = useState<string | null>(null);

  if (agents.length === 0) return null;

  return (
    <svg viewBox={`0 0 ${width} ${height}`} className="w-full max-h-[400px]">
      <defs>
        <marker id="arrow-lateral" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
          <path d="M0,0 L8,3 L0,6" fill="#0080ff40" />
        </marker>
        <marker id="arrow-l4" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
          <path d="M0,0 L8,3 L0,6" fill="#ff664480" />
        </marker>
      </defs>

      {/* Edges */}
      {edges.map((edge, i) => {
        const fi = agentIdx.get(edge.from);
        const ti = agentIdx.get(edge.to);
        if (fi === undefined || ti === undefined) return null;
        const from = positions[fi]!;
        const to = positions[ti]!;
        const isLateral = edge.type === 'lateral';
        const color = isLateral ? '#0080ff40' : '#ff664480';
        // Shorten line to stop at node edge
        const dx = to.x - from.x;
        const dy = to.y - from.y;
        const dist = Math.sqrt(dx * dx + dy * dy) || 1;
        const offset = nodeRadius + 4;
        const x1 = from.x + (dx / dist) * offset;
        const y1 = from.y + (dy / dist) * offset;
        const x2 = to.x - (dx / dist) * offset;
        const y2 = to.y - (dy / dist) * offset;
        return (
          <line
            key={`edge-${i}`}
            x1={x1} y1={y1} x2={x2} y2={y2}
            stroke={color}
            strokeWidth={2}
            strokeDasharray={isLateral ? undefined : '6 3'}
            markerEnd={isLateral ? undefined : 'url(#arrow-l4)'}
          />
        );
      })}

      {/* Nodes */}
      {agents.map((agent, i) => {
        const pos = positions[i]!;
        const isOnline = agent.status === 'online';
        const isHovered = hovered === agent.agent_id;
        const fill = trustColor(agent.trust_level);
        return (
          <g
            key={agent.agent_id}
            className="cursor-pointer"
            onClick={() => onSelect(agent.agent_id)}
            onMouseEnter={() => setHovered(agent.agent_id)}
            onMouseLeave={() => setHovered(null)}
          >
            {/* Glow on hover */}
            {isHovered && (
              <circle cx={pos.x} cy={pos.y} r={nodeRadius + 6} fill="none" stroke={fill} strokeWidth={1} opacity={0.4} />
            )}
            {/* Main circle */}
            <circle
              cx={pos.x} cy={pos.y} r={nodeRadius}
              fill={`${fill}15`}
              stroke={fill}
              strokeWidth={isHovered ? 2.5 : 1.5}
              opacity={isOnline ? 1 : 0.4}
            />
            {/* Status dot */}
            <circle
              cx={pos.x + nodeRadius - 4} cy={pos.y - nodeRadius + 4} r={4}
              fill={isOnline ? '#00ff88' : '#556080'}
            />
            {/* Label */}
            <text
              x={pos.x} y={pos.y + nodeRadius + 16}
              textAnchor="middle"
              fill={isHovered ? '#fff' : '#8892a8'}
              fontSize={11}
              fontFamily="monospace"
            >
              {agent.agent_id.length > 14 ? agent.agent_id.slice(0, 12) + '..' : agent.agent_id}
            </text>
            {/* Role inside node */}
            <text
              x={pos.x} y={pos.y + 4}
              textAnchor="middle"
              fill={fill}
              fontSize={9}
              fontFamily="monospace"
              opacity={0.8}
            >
              {(agent.role ?? 'agent').slice(0, 8)}
            </text>
          </g>
        );
      })}

      {/* Legend */}
      <g transform={`translate(12, ${height - 40})`}>
        <line x1={0} y1={0} x2={20} y2={0} stroke="#0080ff40" strokeWidth={2} />
        <text x={24} y={4} fill="#556080" fontSize={9}>lateral</text>
        <line x1={80} y1={0} x2={100} y2={0} stroke="#ff664480" strokeWidth={2} strokeDasharray="6 3" />
        <text x={104} y={4} fill="#556080" fontSize={9}>l4 destination</text>
      </g>
    </svg>
  );
}

// ── Main Fleet Page ─────────────────────────────────────────
export default function Fleet() {
  const navigate = useNavigate();
  const [agents, setAgents] = useState<TopologyAgent[]>([]);
  const [edges, setEdges] = useState<TopologyEdge[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null);
  const [actionLoading, setActionLoading] = useState(false);
  const [showAddAgent, setShowAddAgent] = useState(false);
  const [showBlueprint, setShowBlueprint] = useState(false);
  const [gatewayPort, setGatewayPort] = useState(42617);

  const load = useCallback(async () => {
    try {
      const topo = await fetchTopology();
      setAgents(topo.agents);
      setEdges(topo.edges);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load topology');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    getStatus().then((s) => {
      const port = (s as unknown as Record<string, unknown>).gateway_port;
      if (typeof port === 'number') setGatewayPort(port);
    }).catch(() => {});
    load();
    const interval = setInterval(load, 10_000);
    return () => clearInterval(interval);
  }, [load]);

  const executeAction = async () => {
    if (!pendingAction) return;
    setActionLoading(true);
    try {
      const { type, agent, level } = pendingAction;
      switch (type) {
        case 'revoke': await revokeAgent(agent.agent_id); break;
        case 'quarantine': await quarantineAgent(agent.agent_id); break;
        case 'disable': await disableAgent(agent.agent_id); break;
        case 'downgrade': if (level !== undefined) await downgradeAgent(agent.agent_id, level); break;
        case 'delete': {
          const result = await deleteAgent(agent.agent_id);
          if (!result.ok) throw new Error(result.error ?? 'Delete failed');
          break;
        }
      }
      setPendingAction(null);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Action failed');
    } finally {
      setActionLoading(false);
    }
  };

  const brokerUrl = `http://127.0.0.1:${gatewayPort}`;

  const confirmMessage = pendingAction
    ? `${pendingAction.type} agent "${pendingAction.agent.agent_id}"${
        pendingAction.type === 'downgrade' ? ` to L${pendingAction.level}` : ''
      }?`
    : '';

  if (loading) {
    return (
      <div className="flex items-center justify-center py-20 animate-fade-in">
        <div className="h-8 w-8 border-2 border-[#0080ff30] border-t-[#0080ff] rounded-full animate-spin" />
      </div>
    );
  }

  return (
    <div className="space-y-6 animate-fade-in">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold text-gradient-blue">{t('ipc.fleet_title')}</h1>
        <div className="flex items-center gap-3">
          <span className="text-sm text-[#556080]">{agents.length} agents</span>
          <button onClick={() => setShowBlueprint(true)} className="px-4 py-1.5 text-sm font-medium text-[#8892a8] rounded-lg border border-[#1a1a3e]/50 hover:bg-[#1a1a3e]/30 transition-colors">
            Blueprint
          </button>
          <button onClick={() => setShowAddAgent(true)} className="btn-electric px-4 py-1.5 text-sm font-medium">
            + Add Agent
          </button>
        </div>
      </div>

      {error && (
        <div className="glass-card p-4 border-red-500/30 text-red-400 text-sm">{error}</div>
      )}

      {/* Communication Graph */}
      {agents.length > 0 && (
        <div className="glass-card p-4">
          <TopologyGraph
            agents={agents}
            edges={edges}
            onSelect={(id) => navigate(`/ipc/fleet/${id}`)}
          />
        </div>
      )}

      {/* Agent Table */}
      {agents.length === 0 ? (
        <div className="glass-card p-12 text-center">
          <p className="text-[#556080]">No agents registered. Deploy a blueprint or add an agent to get started.</p>
        </div>
      ) : (
        <div className="glass-card overflow-hidden">
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-[#1a1a3e]/50 text-[#556080] text-xs uppercase tracking-wider">
                  <th className="text-left px-4 py-3">Agent</th>
                  <th className="text-left px-4 py-3">Role</th>
                  <th className="text-left px-4 py-3">Trust</th>
                  <th className="text-left px-4 py-3">Status</th>
                  <th className="text-left px-4 py-3">Model</th>
                  <th className="text-left px-4 py-3">Last Seen</th>
                  <th className="text-right px-4 py-3">Actions</th>
                </tr>
              </thead>
              <tbody>
                {agents.map((agent) => (
                  <AgentRow
                    key={agent.agent_id}
                    agent={agent}
                    onAction={setPendingAction}
                    onClick={() => navigate(`/ipc/fleet/${agent.agent_id}`)}
                  />
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      <AddAgentDialog open={showAddAgent} onClose={() => setShowAddAgent(false)} onCreated={load} brokerUrl={brokerUrl} />
      <DeployBlueprintDialog open={showBlueprint} onClose={() => setShowBlueprint(false)} onCreated={load} brokerUrl={brokerUrl} />

      <ConfirmDialog
        open={pendingAction !== null}
        title={`${pendingAction?.type ?? ''}`}
        message={confirmMessage}
        confirmLabel={actionLoading ? 'Processing...' : 'Confirm'}
        destructive
        onConfirm={executeAction}
        onCancel={() => setPendingAction(null)}
      />
    </div>
  );
}

// ── Agent Table Row ─────────────────────────────────────────
function AgentRow({
  agent,
  onAction,
  onClick,
}: {
  agent: TopologyAgent;
  onAction: (action: PendingAction) => void;
  onClick: () => void;
}) {
  const [showMenu, setShowMenu] = useState(false);
  const isActive = agent.status === 'online';

  return (
    <tr className="border-b border-[#1a1a3e]/30 hover:bg-[#0080ff05] transition-colors cursor-pointer" onClick={onClick}>
      <td className="px-4 py-3 font-mono text-[#0080ff]">{agent.agent_id}</td>
      <td className="px-4 py-3 text-[#8892a8]">{agent.role ?? '-'}</td>
      <td className="px-4 py-3"><TrustBadge level={agent.trust_level} /></td>
      <td className="px-4 py-3"><StatusBadge status={agent.status} /></td>
      <td className="px-4 py-3 text-[#556080] text-xs">{agent.model ?? '-'}</td>
      <td className="px-4 py-3">
        {agent.last_seen ? <TimeAgo timestamp={agent.last_seen} staleThreshold={300} /> : '-'}
      </td>
      <td className="px-4 py-3 text-right relative" onClick={(e) => e.stopPropagation()}>
        <button
          onClick={() => setShowMenu(!showMenu)}
          className="text-xs text-[#556080] hover:text-white px-2 py-1 rounded hover:bg-[#1a1a3e]/50 transition-colors"
        >
          Actions
        </button>
        {showMenu && (
          <>
            <div className="fixed inset-0 z-10" onClick={() => setShowMenu(false)} />
            <div className="absolute right-4 top-full mt-1 z-20 glass-card py-1 min-w-[140px] shadow-lg">
              {isActive && (
                <>
                  <MenuButton label="Disable" onClick={() => { setShowMenu(false); onAction({ type: 'disable', agent }); }} />
                  <MenuButton label="Quarantine" onClick={() => { setShowMenu(false); onAction({ type: 'quarantine', agent }); }} />
                  {(agent.trust_level ?? 0) < 4 && (
                    <MenuButton label="Downgrade to L4" onClick={() => { setShowMenu(false); onAction({ type: 'downgrade', agent, level: 4 }); }} />
                  )}
                </>
              )}
              <div className="border-t border-[#1a1a3e]/30 my-1" />
              <MenuButton label="Revoke" className="text-red-400 hover:text-red-300" onClick={() => { setShowMenu(false); onAction({ type: 'revoke', agent }); }} />
              <MenuButton label="Delete" className="text-red-400 hover:text-red-300" onClick={() => { setShowMenu(false); onAction({ type: 'delete', agent }); }} />
            </div>
          </>
        )}
      </td>
    </tr>
  );
}

function MenuButton({ label, onClick, className = '' }: { label: string; onClick: () => void; className?: string }) {
  return (
    <button
      onClick={onClick}
      className={`w-full text-left px-3 py-1.5 text-xs hover:bg-[#1a1a3e]/50 transition-colors ${className || 'text-[#8892a8] hover:text-white'}`}
    >
      {label}
    </button>
  );
}
