import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { ArrowRight, Network, Shield, Sparkles } from 'lucide-react';
import { fetchTopology, deleteAgent, revokeAgent, quarantineAgent, disableAgent, downgradeAgent } from '@/lib/ipc-api';
import type { TopologyAgent, TopologyEdge } from '@/lib/ipc-api';
import { getStatus } from '@/lib/api';
import TrustBadge from '@/components/ipc/TrustBadge';
import StatusBadge from '@/components/ipc/StatusBadge';
import TimeAgo from '@/components/ipc/TimeAgo';
import ConfirmDialog from '@/components/ipc/ConfirmDialog';
import AddAgentDialog from '@/components/ipc/AddAgentDialog';
import DeployBlueprintDialog from '@/components/ipc/DeployBlueprintDialog';
import ForceGraph2D from 'react-force-graph-2d';

type ActionType = 'revoke' | 'quarantine' | 'disable' | 'downgrade' | 'delete';

interface PendingAction {
  type: ActionType;
  agent: TopologyAgent;
  level?: number;
}

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

function edgeColor(type: string): string {
  switch (type) {
    case 'lateral': return 'var(--glow-primary)';
    case 'l4_destination': return 'rgba(255, 102, 68, 0.5)';
    case 'message': return 'rgba(0, 255, 136, 0.35)';
    default: return 'rgba(85, 96, 128, 0.3)';
  }
}

function particleColor(type: string): string {
  switch (type) {
    case 'lateral': return '#0080ff';
    case 'l4_destination': return '#ff6644';
    case 'message': return '#00ff88';
    default: return '#556080';
  }
}

interface GraphNode {
  id: string;
  role: string;
  trust_level: number;
  status: string;
  color: string;
  x: number;
  y: number;
  fx?: number;
  fy?: number;
}

interface GraphLink {
  source: string;
  target: string;
  type: string;
  count?: number;
  color: string;
  pColor: string;
}

const TRAFFIC_WINDOW_HOURS = 24;
const TRAFFIC_MIN_COUNT = 2;

function TopologyGraph({
  agents,
  edges,
  showTraffic,
  onSelect,
}: {
  agents: TopologyAgent[];
  edges: TopologyEdge[];
  showTraffic: boolean;
  onSelect: (agentId: string) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const positionsRef = useRef<Map<string, { x: number; y: number; fx?: number; fy?: number }>>(new Map());
  const [hovered, setHovered] = useState<string | null>(null);
  const [dimensions, setDimensions] = useState({ width: 700, height: 380 });

  useEffect(() => {
    if (!containerRef.current) return;
    const ro = new ResizeObserver((entries) => {
      const { width } = entries[0]!.contentRect;
      const ratio = width < 640 ? 0.78 : width < 960 ? 0.62 : 0.5;
      setDimensions({ width, height: Math.min(400, Math.max(250, width * ratio)) });
    });
    ro.observe(containerRef.current);
    return () => ro.disconnect();
  }, []);

  const graphData = useMemo(() => {
    const positions = positionsRef.current;
    const groupedByTrust = new Map<number, TopologyAgent[]>();
    for (const agent of [...agents].sort((a, b) => a.agent_id.localeCompare(b.agent_id))) {
      const trust = agent.trust_level ?? 3;
      const group = groupedByTrust.get(trust) ?? [];
      group.push(agent);
      groupedByTrust.set(trust, group);
    }

    const defaultPositions = new Map<string, { x: number; y: number }>();
    for (const [trust, list] of [...groupedByTrust.entries()].sort((a, b) => a[0] - b[0])) {
      const x = (trust - 2) * 180;
      const yStart = -((list.length - 1) * 70) / 2;
      list.forEach((agent, index) => {
        defaultPositions.set(agent.agent_id, {
          x,
          y: yStart + index * 70,
        });
      });
    }

    const nodes: GraphNode[] = agents.map((a) => {
      const saved = positions.get(a.agent_id);
      const fallback = defaultPositions.get(a.agent_id) ?? { x: 0, y: 0 };
      return {
        id: a.agent_id,
        role: a.role ?? 'agent',
        trust_level: a.trust_level ?? 3,
        status: a.status,
        color: trustColor(a.trust_level),
        x: saved?.x ?? fallback.x,
        y: saved?.y ?? fallback.y,
        fx: saved?.fx ?? fallback.x,
        fy: saved?.fy ?? fallback.y,
      };
    });

    const links: GraphLink[] = edges
      .filter((e) => {
        const hasSource = agents.some((a) => a.agent_id === e.from);
        const hasTarget = agents.some((a) => a.agent_id === e.to);
        return hasSource && hasTarget;
      })
      .map((e) => ({
        source: e.from,
        target: e.to,
        type: e.type,
        count: e.count,
        color: edgeColor(e.type),
        pColor: particleColor(e.type),
      }));

    return { nodes, links };
  }, [agents, edges]);

  if (agents.length === 0) return null;

  return (
    <div ref={containerRef} className="relative">
      <ForceGraph2D
        graphData={graphData}
        width={dimensions.width}
        height={dimensions.height}
        backgroundColor="transparent"
        nodeRelSize={8}
        nodeCanvasObject={(node: GraphNode, ctx: CanvasRenderingContext2D, globalScale: number) => {
          const r = 8;
          const isOnline = node.status === 'online';
          const isHov = hovered === node.id;
          const x = (node as unknown as { x: number }).x;
          const y = (node as unknown as { y: number }).y;

          if (isHov) {
            ctx.beginPath();
            ctx.arc(x, y, r + 4, 0, 2 * Math.PI);
            ctx.strokeStyle = node.color;
            ctx.lineWidth = 1.5 / globalScale;
            ctx.globalAlpha = 0.5;
            ctx.stroke();
            ctx.globalAlpha = 1;
          }

          ctx.beginPath();
          ctx.arc(x, y, r, 0, 2 * Math.PI);
          ctx.fillStyle = `${node.color}20`;
          ctx.fill();
          ctx.strokeStyle = node.color;
          ctx.lineWidth = (isHov ? 2.5 : 1.5) / globalScale;
          ctx.globalAlpha = isOnline ? 1 : 0.35;
          ctx.stroke();
          ctx.globalAlpha = 1;

          ctx.beginPath();
          ctx.arc(x + r - 2, y - r + 2, 2.5, 0, 2 * Math.PI);
          ctx.fillStyle = isOnline ? '#00ff88' : '#556080';
          ctx.fill();

          ctx.font = `${Math.max(3, 7 / globalScale)}px monospace`;
          ctx.textAlign = 'center';
          ctx.textBaseline = 'middle';
          ctx.fillStyle = node.color;
          ctx.globalAlpha = 0.9;
          ctx.fillText(node.role.slice(0, 8), x, y);
          ctx.globalAlpha = 1;

          ctx.font = `${Math.max(3, 8 / globalScale)}px monospace`;
          ctx.fillStyle = isHov ? '#ffffff' : '#8892a8';
          ctx.fillText(
            node.id.length > 14 ? `${node.id.slice(0, 12)}..` : node.id,
            x,
            y + r + 8 / globalScale,
          );
        }}
        nodePointerAreaPaint={(node: GraphNode, color: string, ctx: CanvasRenderingContext2D) => {
          const x = (node as unknown as { x: number }).x;
          const y = (node as unknown as { y: number }).y;
          ctx.beginPath();
          ctx.arc(x, y, 12, 0, 2 * Math.PI);
          ctx.fillStyle = color;
          ctx.fill();
        }}
        onNodeClick={(node: GraphNode) => onSelect(node.id)}
        onNodeHover={(node: GraphNode | null) => setHovered(node?.id ?? null)}
        onNodeDragEnd={(node: GraphNode & { x: number; y: number }) => {
          (node as unknown as { fx: number }).fx = node.x;
          (node as unknown as { fy: number }).fy = node.y;
          positionsRef.current.set(node.id, { x: node.x, y: node.y, fx: node.x, fy: node.y });
        }}
        linkColor={(link: GraphLink) => link.color}
        linkWidth={(link: GraphLink) => {
          const src = typeof link.source === 'object' ? (link.source as GraphNode).id : link.source;
          const tgt = typeof link.target === 'object' ? (link.target as GraphNode).id : link.target;
          const connected = hovered && (src === hovered || tgt === hovered);
          if (link.type === 'message') {
            const weight = link.count ? Math.min(4, 1 + Math.log2(Math.max(link.count, 1))) : 1.4;
            return connected ? weight + 1 : weight;
          }
          return connected ? 2.5 : 1.4;
        }}
        linkDirectionalArrowLength={6}
        linkDirectionalArrowRelPos={0.9}
        linkDirectionalArrowColor={(link: GraphLink) => link.pColor}
        linkLineDash={(link: GraphLink) => (link.type === 'l4_destination' ? [4, 2] : null)}
        linkDirectionalParticles={() => 0}
        enableZoomInteraction
        enablePanInteraction
        enableNodeDrag
      />
      <div className="pointer-events-none absolute bottom-2 left-3 flex items-center gap-4 text-[9px] text-[var(--text-secondary)]">
        <span className="flex items-center gap-1">
          <span className="inline-block w-4 h-[2px]" style={{ background: 'rgba(0,128,255,0.5)' }} /> lateral
        </span>
        <span className="flex items-center gap-1">
          <span className="inline-block w-4 h-[2px] border-t-2 border-dashed" style={{ borderColor: 'rgba(255,102,68,0.5)' }} /> l4 dest
        </span>
        {showTraffic && (
          <span className="flex items-center gap-1">
            <span className="inline-block w-4 h-[2px]" style={{ background: 'rgba(0,255,136,0.35)' }} /> traffic
          </span>
        )}
      </div>
    </div>
  );
}

function FleetStatCard({
  label,
  value,
  caption,
}: {
  label: string;
  value: string;
  caption: string;
}) {
  return (
    <div className="rounded-3xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-4">
      <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">{label}</p>
      <p className="mt-2 text-2xl font-semibold tracking-tight text-[var(--text-primary)]">{value}</p>
      <p className="mt-1 text-xs text-[var(--text-muted)]">{caption}</p>
    </div>
  );
}

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
  const [showTraffic, setShowTraffic] = useState(false);
  const [showEphemeral, setShowEphemeral] = useState(false);

  const load = useCallback(async () => {
    try {
      const topo = await fetchTopology({
        includeTraffic: showTraffic,
        includeEphemeral: showEphemeral,
        trafficHours: TRAFFIC_WINDOW_HOURS,
        trafficMinCount: TRAFFIC_MIN_COUNT,
      });
      setAgents(topo.agents);
      setEdges(topo.edges);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load topology');
    } finally {
      setLoading(false);
    }
  }, [showEphemeral, showTraffic]);

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
  const onlineAgents = agents.filter((agent) => agent.status === 'online').length;
  const trustCounts = agents.reduce<Record<number, number>>((acc, agent) => {
    const level = agent.trust_level ?? 3;
    acc[level] = (acc[level] ?? 0) + 1;
    return acc;
  }, {});

  const confirmMessage = pendingAction
    ? `${pendingAction.type} agent "${pendingAction.agent.agent_id}"${
      pendingAction.type === 'downgrade' ? ` to L${pendingAction.level}` : ''
    }?`
    : '';

  if (loading) {
    return (
      <div className="flex items-center justify-center py-20 animate-fade-in">
        <div className="h-8 w-8 border-2 border-[var(--glow-primary)] border-t-[var(--accent-primary)] rounded-full animate-spin" />
      </div>
    );
  }

  return (
    <div className="space-y-6 p-4 animate-fade-in md:p-6">
      <div className="animate-panel-reveal relative overflow-hidden rounded-[28px] border border-[var(--border-default)] bg-[linear-gradient(135deg,var(--glow-primary),transparent_35%),var(--bg-card)] px-5 py-5 md:px-6 md:py-6">
        <div className="absolute inset-x-10 top-0 h-px bg-gradient-to-r from-transparent via-[var(--accent-primary)]/70 to-transparent" />
        <div className="flex flex-col gap-5 xl:flex-row xl:items-end xl:justify-between">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.28em] text-[var(--text-placeholder)]">
              Fleet Scope
            </p>
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <h1 className="text-3xl font-semibold tracking-tight text-[var(--text-primary)]">
                Broker Fleet Map
              </h1>
              <span className="rounded-full bg-[var(--accent-primary)]/10 px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide text-[var(--accent-primary)]">
                {agents.length} agents
              </span>
            </div>
            <p className="mt-2 max-w-2xl text-sm text-[var(--text-muted)]">
              Topology, trust posture, runtime health, and direct operator actions across the broker-connected fleet.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              onClick={() => setShowBlueprint(true)}
              className="btn-secondary inline-flex items-center gap-2 px-4 py-2 text-sm"
            >
              <Sparkles className="h-4 w-4" />
              Blueprint
            </button>
            <button
              onClick={() => setShowAddAgent(true)}
              className="btn-primary inline-flex items-center gap-2 px-4 py-2 text-sm"
            >
              + Add Agent
            </button>
          </div>
        </div>
      </div>

      {error && (
        <div className="glass-card border-red-500/30 p-4 text-sm text-red-400">{error}</div>
      )}

      <div className="stagger-children grid gap-3 md:grid-cols-2 xl:grid-cols-4">
        <FleetStatCard label="Online" value={`${onlineAgents}/${agents.length || 0}`} caption="reachable agents" />
        <FleetStatCard label="Edges" value={`${edges.length}`} caption={showTraffic ? 'topology + recent traffic' : 'declared topology only'} />
        <FleetStatCard label="Broker URL" value={`:${gatewayPort}`} caption={brokerUrl} />
        <FleetStatCard label="Trust Mix" value={`L0:${trustCounts[0] ?? 0} L4:${trustCounts[4] ?? 0}`} caption="low and high trust edges" />
      </div>

      {agents.length > 0 && (
        <div className="glass-card animate-panel-reveal overflow-hidden" style={{ minHeight: 300 }}>
          <div className="border-b border-[var(--bg-hover)] px-5 py-4">
            <div className="flex flex-col gap-4 xl:flex-row xl:items-center xl:justify-between">
              <div>
                <div className="flex items-center gap-2">
                  <Network className="h-4 w-4 text-[var(--accent-primary)]" />
                  <div className="text-sm font-medium text-[var(--text-primary)]">
                    {showTraffic ? 'Observed Traffic' : 'Policy Topology'}
                  </div>
                </div>
                <div className="mt-1 text-xs text-[var(--text-secondary)]">
                  {showTraffic
                    ? `Recent IPC traffic, last ${TRAFFIC_WINDOW_HOURS}h, count ≥ ${TRAFFIC_MIN_COUNT}`
                    : 'Declared communication topology only. Historical traffic hidden to keep the graph readable.'}
                </div>
              </div>
              <div className="flex flex-wrap items-center gap-2 text-xs">
                <button
                  onClick={() => setShowTraffic((v) => !v)}
                  className={`rounded-full border px-3 py-1.5 font-semibold uppercase tracking-wide transition-colors ${
                    showTraffic
                      ? 'border-[#00ff88]/40 bg-[#00ff8815] text-[#00ff88]'
                      : 'border-[var(--bg-secondary)] text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                  }`}
                >
                  {showTraffic ? 'Hide Traffic' : 'Show Traffic'}
                </button>
                <button
                  onClick={() => setShowEphemeral((v) => !v)}
                  className={`rounded-full border px-3 py-1.5 font-semibold uppercase tracking-wide transition-colors ${
                    showEphemeral
                      ? 'border-[#ff6644]/40 bg-[#ff664415] text-[#ff9b7a]'
                      : 'border-[var(--bg-secondary)] text-[var(--text-muted)] hover:bg-[var(--bg-hover)]'
                  }`}
                >
                  {showEphemeral ? 'Hide Ephemeral' : 'Show Ephemeral'}
                </button>
              </div>
            </div>
          </div>
          <div className="p-2 md:p-3">
            <TopologyGraph
              agents={agents}
              edges={edges}
              showTraffic={showTraffic}
              onSelect={(id) => navigate(`/ipc/fleet/${id}`)}
            />
          </div>
        </div>
      )}

      <div className="grid gap-6 xl:grid-cols-[1.2fr_0.8fr]">
        <div className="space-y-6">
          {agents.length === 0 ? (
            <div className="glass-card p-12 text-center">
              <p className="text-[var(--text-secondary)]">No agents registered. Deploy a blueprint or add an agent to get started.</p>
            </div>
          ) : (
            <div className="glass-card animate-panel-reveal overflow-hidden">
              <div className="border-b border-[var(--bg-secondary)] px-5 py-4">
                <div className="flex items-center gap-2">
                  <Shield className="h-4 w-4 text-[var(--accent-primary)]" />
                  <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">Agent Registry</h2>
                </div>
              </div>
              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-[var(--bg-secondary)] text-[var(--text-secondary)] text-xs uppercase tracking-wider">
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
        </div>

        <div className="space-y-6">
          <div className="glass-card animate-panel-reveal overflow-hidden">
            <div className="border-b border-[var(--bg-secondary)] px-5 py-4">
              <div className="flex items-center gap-2">
                <Sparkles className="h-4 w-4 text-[var(--accent-primary)]" />
                <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">Operator Shortcuts</h2>
              </div>
            </div>
            <div className="space-y-3 px-5 py-5">
              <button
                onClick={() => navigate('/ipc/activity')}
                className="flex w-full items-center justify-between rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3 text-left transition-all duration-300 hover:-translate-y-0.5 hover:border-[var(--accent-primary)]/30 hover:shadow-[0_10px_24px_var(--glow-primary)]"
              >
                <div>
                  <p className="text-sm font-medium text-[var(--text-primary)]">Open Activity Feed</p>
                  <p className="mt-1 text-xs text-[var(--text-muted)]">Inspect cross-surface movement and recent broker events.</p>
                </div>
                <ArrowRight className="h-4 w-4 text-[var(--text-muted)]" />
              </button>
              <button
                onClick={() => navigate('/agents')}
                className="flex w-full items-center justify-between rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3 text-left transition-all duration-300 hover:-translate-y-0.5 hover:border-[var(--accent-primary)]/30 hover:shadow-[0_10px_24px_var(--glow-primary)]"
              >
                <div>
                  <p className="text-sm font-medium text-[var(--text-primary)]">Open Workbench</p>
                  <p className="mt-1 text-xs text-[var(--text-muted)]">Jump from fleet scope into the live agent chat workbench.</p>
                </div>
                <ArrowRight className="h-4 w-4 text-[var(--text-muted)]" />
              </button>
            </div>
          </div>

          <div className="glass-card animate-panel-reveal overflow-hidden">
            <div className="border-b border-[var(--bg-secondary)] px-5 py-4">
              <div className="flex items-center gap-2">
                <Shield className="h-4 w-4 text-[var(--accent-primary)]" />
                <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">Trust Posture</h2>
              </div>
            </div>
            <div className="grid gap-3 px-5 py-5 md:grid-cols-2 xl:grid-cols-1">
              {[0, 1, 2, 3, 4].map((level) => (
                <div key={level} className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3">
                  <div className="flex items-center justify-between gap-3">
                    <span className="text-sm font-medium text-[var(--text-primary)]">Trust L{level}</span>
                    <span className="text-sm text-[var(--text-muted)]">{trustCounts[level] ?? 0}</span>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>

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
    <tr className="cursor-pointer border-b border-[var(--bg-hover)] transition-colors hover:bg-[var(--glow-secondary)]" onClick={onClick}>
      <td className="px-4 py-3">
        <div>
          <div className="font-mono text-[var(--accent-primary)]">{agent.agent_id}</div>
          <div className="mt-1 text-[11px] text-[var(--text-muted)]">
            {(agent.channels?.length ?? 0)} channels
          </div>
        </div>
      </td>
      <td className="px-4 py-3 text-[var(--text-muted)]">{agent.role ?? '-'}</td>
      <td className="px-4 py-3"><TrustBadge level={agent.trust_level} /></td>
      <td className="px-4 py-3"><StatusBadge status={agent.status} /></td>
      <td className="px-4 py-3 text-xs text-[var(--text-secondary)]">{agent.model ?? '-'}</td>
      <td className="px-4 py-3">
        {agent.last_seen ? <TimeAgo timestamp={agent.last_seen} staleThreshold={300} /> : '-'}
      </td>
      <td className="relative px-4 py-3 text-right" onClick={(e) => e.stopPropagation()}>
        <button
          onClick={() => setShowMenu(!showMenu)}
          className="rounded-lg px-2 py-1 text-xs text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-secondary)] hover:text-[var(--text-primary)]"
        >
          Actions
        </button>
        {showMenu && (
          <>
            <div className="fixed inset-0 z-10" onClick={() => setShowMenu(false)} />
            <div className="absolute right-4 top-full z-20 mt-1 min-w-[140px] glass-card py-1 shadow-lg">
              {isActive && (
                <>
                  <MenuButton label="Disable" onClick={() => { setShowMenu(false); onAction({ type: 'disable', agent }); }} />
                  <MenuButton label="Quarantine" onClick={() => { setShowMenu(false); onAction({ type: 'quarantine', agent }); }} />
                  {(agent.trust_level ?? 0) < 4 && (
                    <MenuButton label="Downgrade to L4" onClick={() => { setShowMenu(false); onAction({ type: 'downgrade', agent, level: 4 }); }} />
                  )}
                </>
              )}
              <div className="my-1 border-t border-[var(--bg-hover)]" />
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
      className={`w-full px-3 py-1.5 text-left text-xs transition-colors hover:bg-[var(--bg-secondary)] ${className || 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'}`}
    >
      {label}
    </button>
  );
}
