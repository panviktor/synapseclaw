// TOML config generator for Phase 3.6 Agent Provisioning UI
// Config is generated client-side — API keys never touch the broker (AD-1)
// Fields verified against src/config/schema.rs top-level Config struct

import { getProviderById } from './ipc-providers';
import { getChannelById } from './ipc-channels';

export interface AgentConfigInputs {
  agentId: string;
  role: string;
  trustLevel: number;
  providerId: string;
  apiKey: string;
  model: string;
  baseUrl: string;
  channelId: string;
  channelValues: Record<string, string>;
  brokerUrl: string;
  gatewayPort: number;
  systemPrompt: string;
  brokerToken?: string;
}

function escapeToml(s: string): string {
  return s.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

function tomlList(items: string[]): string {
  if (items.length === 0) return '[]';
  return `[${items.map((i) => `"${escapeToml(i.trim())}"`).join(', ')}]`;
}

/** Fields that should be emitted as bare integers (not quoted strings). */
const NUMERIC_FIELDS = new Set(['port']);

export function generateAgentConfig(inputs: AgentConfigInputs): string {
  const provider = getProviderById(inputs.providerId);
  const lines: string[] = [];

  lines.push('# ZeroClaw Agent Configuration');
  lines.push(`# Generated for agent: ${inputs.agentId}`);
  lines.push('');

  // ── Provider (top-level fields per schema.rs:74-86) ──
  const providerName = inputs.providerId === 'custom' ? `custom:${inputs.baseUrl}` : inputs.providerId;
  lines.push(`default_provider = "${escapeToml(providerName)}"`);
  lines.push(`default_model = "${escapeToml(inputs.model)}"`);

  // api_key is a top-level field (schema.rs:74)
  if (inputs.apiKey) {
    const envVar = provider?.env_var ?? `${inputs.providerId.toUpperCase().replace(/-/g, '_')}_API_KEY`;
    lines.push(`# Or set env: ${envVar}`);
    lines.push(`api_key = "${escapeToml(inputs.apiKey)}"`);
  }

  // api_url is a top-level field (schema.rs:76) — for local/custom providers
  if (inputs.baseUrl) {
    lines.push(`api_url = "${escapeToml(inputs.baseUrl)}"`);
  }

  lines.push('');

  // ── Gateway ──
  lines.push('[gateway]');
  lines.push(`port = ${inputs.gatewayPort}`);
  lines.push('host = "127.0.0.1"');
  lines.push('require_pairing = true');
  lines.push('');

  // ── IPC ──
  lines.push('[agents_ipc]');
  lines.push('enabled = true');
  lines.push(`broker_url = "${escapeToml(inputs.brokerUrl)}"`);
  lines.push(`gateway_url = "http://127.0.0.1:${inputs.gatewayPort}"`);
  if (inputs.brokerToken) {
    lines.push(`broker_token = "${escapeToml(inputs.brokerToken)}"`);
  }
  lines.push(`trust_level = ${inputs.trustLevel}`);
  lines.push(`role = "${escapeToml(inputs.role)}"`);
  lines.push(`agent_id = "${escapeToml(inputs.agentId)}"`);
  lines.push('request_timeout_secs = 10');
  lines.push('max_messages_per_hour = 60');
  lines.push('');

  // ── Channel ──
  if (inputs.channelId && inputs.channelId !== 'none') {
    const channel = getChannelById(inputs.channelId);
    if (channel) {
      lines.push(`[channels_config.${inputs.channelId}]`);
      for (const field of channel.fields) {
        const value = inputs.channelValues[field.key];
        if (!value) continue;
        if (field.type === 'list') {
          const items = value.split(',').map((s) => s.trim()).filter(Boolean);
          lines.push(`${field.key} = ${tomlList(items)}`);
        } else if (NUMERIC_FIELDS.has(field.key)) {
          const num = parseInt(value, 10);
          if (!isNaN(num)) {
            lines.push(`${field.key} = ${num}`);
          }
        } else {
          lines.push(`${field.key} = "${escapeToml(value)}"`);
        }
      }
      lines.push('');
    }
  }

  return lines.join('\n');
}

/** Generate instructions.md content from system prompt. */
export function generateInstructionsMd(systemPrompt: string): string {
  return `# Agent Instructions\n\n${systemPrompt}\n`;
}

export function downloadAsFile(filename: string, content: string): void {
  const blob = new Blob([content], { type: 'application/toml' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}
