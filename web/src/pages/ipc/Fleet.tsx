import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
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
import ForceGraph2D from 'react-force-graph-2d';

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

// ── Edge colors by type ─────────────────────────────────────
function edgeColor(type: string): string {
  switch (type) {
    case 'lateral': return 'rgba(0, 128, 255, 0.5)';
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

// ── Graph node/link types ───────────────────────────────────
interface GraphNode {
  id: string;
  role: string;
  trust_level: number;
  status: string;
  color: string;
  // Position fields set by force simulation and position preservation
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

// ── Force Graph Topology ────────────────────────────────────
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

  // Measure container
  useEffect(() => {
    if (!containerRef.current) return;
    const ro = new ResizeObserver((entries) => {
      const { width } = entries[0]!.contentRect;
      setDimensions({ width, height: Math.min(380, Math.max(280, width * 0.5)) });
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
        // Nodes
        nodeRelSize={8}
        nodeCanvasObject={(node: GraphNode, ctx: CanvasRenderingContext2D, globalScale: number) => {
          const r = 8;
          const isOnline = node.status === 'online';
          const isHov = hovered === node.id;
          const x = (node as unknown as { x: number }).x;
          const y = (node as unknown as { y: number }).y;

          // Glow ring on hover
          if (isHov) {
            ctx.beginPath();
            ctx.arc(x, y, r + 4, 0, 2 * Math.PI);
            ctx.strokeStyle = node.color;
            ctx.lineWidth = 1.5 / globalScale;
            ctx.globalAlpha = 0.5;
            ctx.stroke();
            ctx.globalAlpha = 1;
          }

          // Main circle
          ctx.beginPath();
          ctx.arc(x, y, r, 0, 2 * Math.PI);
          ctx.fillStyle = node.color + '20';
          ctx.fill();
          ctx.strokeStyle = node.color;
          ctx.lineWidth = (isHov ? 2.5 : 1.5) / globalScale;
          ctx.globalAlpha = isOnline ? 1 : 0.35;
          ctx.stroke();
          ctx.globalAlpha = 1;

          // Status dot
          ctx.beginPath();
          ctx.arc(x + r - 2, y - r + 2, 2.5, 0, 2 * Math.PI);
          ctx.fillStyle = isOnline ? '#00ff88' : '#556080';
          ctx.fill();

          // Role label inside
          ctx.font = `${Math.max(3, 7 / globalScale)}px monospace`;
          ctx.textAlign = 'center';
          ctx.textBaseline = 'middle';
          ctx.fillStyle = node.color;
          ctx.globalAlpha = 0.9;
          ctx.fillText(node.role.slice(0, 8), x, y);
          ctx.globalAlpha = 1;

          // Agent ID below
          ctx.font = `${Math.max(3, 8 / globalScale)}px monospace`;
          ctx.fillStyle = isHov ? '#ffffff' : '#8892a8';
          ctx.fillText(
            node.id.length > 14 ? node.id.slice(0, 12) + '..' : node.id,
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
          // Pin dragged node and save position
          (node as unknown as { fx: number }).fx = node.x;
          (node as unknown as { fy: number }).fy = node.y;
          positionsRef.current.set(node.id, { x: node.x, y: node.y, fx: node.x, fy: node.y });
        }}
        // Links
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
        linkLineDash={(link: GraphLink) => link.type === 'l4_destination' ? [4, 2] : null}
        linkDirectionalParticles={() => 0}
        enableZoomInteraction={true}
        enablePanInteraction={true}
        enableNodeDrag={true}
      />
      {/* Legend */}
      <div className="absolute bottom-2 left-3 flex items-center gap-4 text-[9px] text-[#556080] pointer-events-none">
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
        <div>
          <h1 className="text-2xl font-bold text-gradient-blue">{t('ipc.fleet_title')}</h1>
          <p className="text-xs text-[#556080] mt-1">{t('ipc.fleet_subtitle')}</p>
        </div>
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
        <div className="glass-card p-2 overflow-hidden" style={{ minHeight: 300 }}>
          <div className="flex items-center justify-between gap-4 px-3 py-2 border-b border-[#1a1a3e]/40">
            <div>
              <div className="text-sm font-medium text-white">
                {showTraffic ? 'Observed Traffic' : 'Policy Topology'}
              </div>
              <div className="text-xs text-[#556080]">
                {showTraffic
                  ? `Recent IPC traffic, last ${TRAFFIC_WINDOW_HOURS}h, count ≥ ${TRAFFIC_MIN_COUNT}`
                  : 'Declared communication topology only. Historical traffic hidden to keep the graph readable.'}
              </div>
            </div>
            <div className="flex items-center gap-2 text-xs">
              <button
                onClick={() => setShowTraffic((v) => !v)}
                className={`px-3 py-1 rounded-md border transition-colors ${
                  showTraffic
                    ? 'border-[#00ff88]/40 bg-[#00ff8815] text-[#00ff88]'
                    : 'border-[#1a1a3e]/50 text-[#8892a8] hover:bg-[#1a1a3e]/30'
                }`}
              >
                {showTraffic ? 'Hide Traffic' : 'Show Traffic'}
              </button>
              <button
                onClick={() => setShowEphemeral((v) => !v)}
                className={`px-3 py-1 rounded-md border transition-colors ${
                  showEphemeral
                    ? 'border-[#ff6644]/40 bg-[#ff664415] text-[#ff9b7a]'
                    : 'border-[#1a1a3e]/50 text-[#8892a8] hover:bg-[#1a1a3e]/30'
                }`}
              >
                {showEphemeral ? 'Hide Ephemeral' : 'Show Ephemeral'}
              </button>
            </div>
          </div>
          <TopologyGraph
            agents={agents}
            edges={edges}
            showTraffic={showTraffic}
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
