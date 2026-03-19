// IPC admin API client for Phase 3.5 operator UI

import { apiFetch } from './api';
import type {
  IpcAgent,
  IpcAgentDetail,
  IpcMessage,
  IpcSpawnRun,
  IpcAuditEvent,
  MessagesFilter,
  SpawnRunsFilter,
  AuditFilter,
} from '../types/ipc';

// ---------------------------------------------------------------------------
// Read endpoints (admin, localhost-only)
// ---------------------------------------------------------------------------

export async function fetchFleet(): Promise<IpcAgent[]> {
  const data = await apiFetch<{ agents: IpcAgent[] }>('/admin/ipc/agents');
  return data.agents;
}

export async function fetchAgentDetail(agentId: string): Promise<IpcAgentDetail> {
  return apiFetch<IpcAgentDetail>(
    `/admin/ipc/agents/${encodeURIComponent(agentId)}/detail`,
  );
}

export async function fetchMessages(filters: MessagesFilter = {}): Promise<IpcMessage[]> {
  const params = new URLSearchParams();
  if (filters.agent_id) params.set('agent_id', filters.agent_id);
  if (filters.session_id) params.set('session_id', filters.session_id);
  if (filters.kind) params.set('kind', filters.kind);
  if (filters.quarantine !== undefined) params.set('quarantine', String(filters.quarantine));
  if (filters.dismissed !== undefined) params.set('dismissed', String(filters.dismissed));
  if (filters.lane) params.set('lane', filters.lane);
  if (filters.from_ts !== undefined) params.set('from_ts', String(filters.from_ts));
  if (filters.to_ts !== undefined) params.set('to_ts', String(filters.to_ts));
  if (filters.limit !== undefined) params.set('limit', String(filters.limit));
  if (filters.offset !== undefined) params.set('offset', String(filters.offset));
  const qs = params.toString();
  const data = await apiFetch<{ messages: IpcMessage[] }>(
    `/admin/ipc/messages${qs ? `?${qs}` : ''}`,
  );
  return data.messages;
}

export async function fetchSpawnRuns(filters: SpawnRunsFilter = {}): Promise<IpcSpawnRun[]> {
  const params = new URLSearchParams();
  if (filters.status) params.set('status', filters.status);
  if (filters.parent_id) params.set('parent_id', filters.parent_id);
  if (filters.from_ts !== undefined) params.set('from_ts', String(filters.from_ts));
  if (filters.to_ts !== undefined) params.set('to_ts', String(filters.to_ts));
  if (filters.limit !== undefined) params.set('limit', String(filters.limit));
  if (filters.offset !== undefined) params.set('offset', String(filters.offset));
  const qs = params.toString();
  const data = await apiFetch<{ spawn_runs: IpcSpawnRun[] }>(
    `/admin/ipc/spawn-runs${qs ? `?${qs}` : ''}`,
  );
  return data.spawn_runs;
}

export async function fetchAudit(filters: AuditFilter = {}): Promise<IpcAuditEvent[]> {
  const params = new URLSearchParams();
  if (filters.agent_id) params.set('agent_id', filters.agent_id);
  if (filters.event_type) params.set('event_type', filters.event_type);
  if (filters.from_ts !== undefined) params.set('from_ts', String(filters.from_ts));
  if (filters.to_ts !== undefined) params.set('to_ts', String(filters.to_ts));
  if (filters.search) params.set('search', filters.search);
  if (filters.limit !== undefined) params.set('limit', String(filters.limit));
  if (filters.offset !== undefined) params.set('offset', String(filters.offset));
  const qs = params.toString();
  const data = await apiFetch<{ events: IpcAuditEvent[] }>(
    `/admin/ipc/audit${qs ? `?${qs}` : ''}`,
  );
  return data.events;
}

// ---------------------------------------------------------------------------
// Write endpoints (admin actions)
// ---------------------------------------------------------------------------

export function revokeAgent(agentId: string): Promise<{ ok: boolean; found: boolean; tokens_revoked: number }> {
  return apiFetch('/admin/ipc/revoke', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId }),
  });
}

export function quarantineAgent(agentId: string): Promise<{ ok: boolean; found: boolean; messages_quarantined: number }> {
  return apiFetch('/admin/ipc/quarantine', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId }),
  });
}

export function disableAgent(agentId: string): Promise<{ ok: boolean; found: boolean }> {
  return apiFetch('/admin/ipc/disable', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId }),
  });
}

export function downgradeAgent(agentId: string, newLevel: number): Promise<{ ok: boolean; old_level: number; new_level: number }> {
  return apiFetch('/admin/ipc/downgrade', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId, new_level: newLevel }),
  });
}

export function promoteMessage(messageId: number, toAgent: string): Promise<{ promoted: boolean; new_message_id: number }> {
  return apiFetch('/admin/ipc/promote', {
    method: 'POST',
    body: JSON.stringify({ message_id: messageId, to_agent: toAgent }),
  });
}

export function dismissMessage(messageId: number): Promise<{ ok: boolean; dismissed: boolean }> {
  return apiFetch('/admin/ipc/dismiss-message', {
    method: 'POST',
    body: JSON.stringify({ message_id: messageId }),
  });
}

export function verifyAuditChain(): Promise<{ ok: boolean; verified?: number; error?: string }> {
  return apiFetch('/admin/ipc/audit/verify', { method: 'POST' });
}

export function createPaircode(agentId: string, trustLevel: number, role: string): Promise<{
  success: boolean;
  pairing_code: string;
  message: string;
}> {
  return apiFetch('/admin/paircode/new', {
    method: 'POST',
    body: JSON.stringify({ agent_id: agentId, trust_level: trustLevel, role }),
  });
}

/** Pair with a pairing code to get a broker_token for an IPC agent.
 *  Unlike the UI `pair()` in api.ts, this does NOT store the token as the UI auth token. */
export async function pairAgent(code: string): Promise<string> {
  const response = await fetch('/pair', {
    method: 'POST',
    headers: { 'X-Pairing-Code': code },
  });
  if (!response.ok) {
    const text = await response.text().catch(() => '');
    throw new Error(`Agent pairing failed (${response.status}): ${text || response.statusText}`);
  }
  const data = (await response.json()) as { token?: string; paired?: boolean };
  if (!data.token) throw new Error('Pairing succeeded but no token returned');
  return data.token;
}

// ---------------------------------------------------------------------------
// Provisioning (Phase 3.8 Step 11)
// ---------------------------------------------------------------------------

export interface ProvisioningStatus {
  enabled: boolean;
  armed: boolean;
  remaining_secs: number;
  mode: string;
}

export function getProvisioningStatus(): Promise<ProvisioningStatus> {
  return apiFetch<ProvisioningStatus>('/admin/provisioning/status');
}

export function armProvisioning(minutes: number = 30): Promise<{ ok: boolean; armed: boolean; minutes: number; mode: string }> {
  return apiFetch('/admin/provisioning/arm', {
    method: 'POST',
    body: JSON.stringify({ minutes }),
  });
}

export function provisionCreate(instance: string, configToml: string, instructionsMd?: string): Promise<{ ok: boolean; instance: string; config_path: string }> {
  return apiFetch('/admin/provisioning/create', {
    method: 'POST',
    body: JSON.stringify({ instance, config_toml: configToml, instructions_md: instructionsMd }),
  });
}

export function provisionInstall(instance: string): Promise<{ ok: boolean; instance: string; stdout?: string }> {
  return apiFetch('/admin/provisioning/install', {
    method: 'POST',
    body: JSON.stringify({ instance }),
  });
}

export function provisionStart(instance: string): Promise<{ ok: boolean; instance: string }> {
  return apiFetch('/admin/provisioning/start', {
    method: 'POST',
    body: JSON.stringify({ instance }),
  });
}

export function provisionStop(instance: string): Promise<{ ok: boolean; instance: string }> {
  return apiFetch('/admin/provisioning/stop', {
    method: 'POST',
    body: JSON.stringify({ instance }),
  });
}

export function provisionUninstall(instance: string): Promise<{ ok: boolean; instance: string }> {
  return apiFetch('/admin/provisioning/uninstall', {
    method: 'POST',
    body: JSON.stringify({ instance }),
  });
}

export function patchBrokerConfig(patchToml: string): Promise<{ ok: boolean; message: string }> {
  return apiFetch('/admin/provisioning/patch-broker', {
    method: 'POST',
    body: JSON.stringify({ patch_toml: patchToml }),
  });
}

export function getUsedPorts(): Promise<{ ports: number[]; next_available: number }> {
  return apiFetch('/admin/provisioning/used-ports');
}

// ---------------------------------------------------------------------------
// Topology (merged agent list + communication graph)
// ---------------------------------------------------------------------------

export interface TopologyAgent {
  agent_id: string;
  role: string | null;
  trust_level: number | null;
  status: string;
  gateway_url: string | null;
  model: string | null;
  last_seen: number | null;
  uptime_seconds?: number | null;
  channels?: string[];
  public_key?: string | null;
  source?: string;
}

export interface TopologyEdge {
  from: string;
  to: string;
  type: 'lateral' | 'l4_destination' | 'message';
  alias?: string;
  count?: number;
}

export interface Topology {
  agents: TopologyAgent[];
  edges: TopologyEdge[];
}

export function fetchTopology(): Promise<Topology> {
  return apiFetch<Topology>('/admin/provisioning/topology');
}

/**
 * Full delete flow: arm → stop → uninstall service → remove config dir.
 */
export async function deleteAgent(instance: string): Promise<{ ok: boolean; error?: string }> {
  try {
    const status = await getProvisioningStatus();
    if (!status.enabled) return { ok: false, error: 'Provisioning disabled in broker config' };
    if (!status.armed) await armProvisioning(30);
    // Best-effort stop (may already be stopped)
    try { await provisionStop(instance); } catch { /* ignore */ }
    await provisionUninstall(instance);
    return { ok: true };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : 'Delete failed' };
  }
}

/**
 * Full deploy flow: arm → create config → install service → start.
 * Returns step-by-step results. Stops on first error.
 */
export async function deployAgent(
  instance: string,
  configToml: string,
  instructionsMd?: string,
): Promise<{ step: string; ok: boolean; error?: string }[]> {
  const results: { step: string; ok: boolean; error?: string }[] = [];

  // 1. Check status and arm if needed
  try {
    const status = await getProvisioningStatus();
    if (!status.enabled) {
      results.push({ step: 'check', ok: false, error: 'Provisioning is disabled in broker config (gateway.ui_provisioning.enabled = false)' });
      return results;
    }
    if (!status.armed) {
      await armProvisioning(30);
    }
    results.push({ step: 'arm', ok: true });
  } catch (e) {
    results.push({ step: 'arm', ok: false, error: e instanceof Error ? e.message : 'Failed to arm' });
    return results;
  }

  // 2. Create config on disk
  try {
    await provisionCreate(instance, configToml, instructionsMd);
    results.push({ step: 'create', ok: true });
  } catch (e) {
    results.push({ step: 'create', ok: false, error: e instanceof Error ? e.message : 'Failed to create config' });
    return results;
  }

  // 3. Install service (may fail if mode=config_only — that's ok)
  try {
    await provisionInstall(instance);
    results.push({ step: 'install', ok: true });
  } catch (e) {
    const msg = e instanceof Error ? e.message : '';
    if (msg.includes('config_only')) {
      results.push({ step: 'install', ok: false, error: 'Mode is config_only — service not installed. Start manually.' });
      return results;
    }
    results.push({ step: 'install', ok: false, error: msg || 'Failed to install service' });
    return results;
  }

  // 4. Start service
  try {
    await provisionStart(instance);
    results.push({ step: 'start', ok: true });
  } catch (e) {
    results.push({ step: 'start', ok: false, error: e instanceof Error ? e.message : 'Failed to start service' });
  }

  return results;
}

// ---------------------------------------------------------------------------
// Availability check
// ---------------------------------------------------------------------------

/** Check if IPC admin endpoints are accessible (localhost-only). */
export async function checkIpcAccess(): Promise<boolean> {
  try {
    await apiFetch('/admin/ipc/agents');
    return true;
  } catch {
    return false;
  }
}
