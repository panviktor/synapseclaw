import { useState } from 'react';
import { createPortal } from 'react-dom';
import { FLEET_BLUEPRINTS, getPresetById, type FleetBlueprint, type BlueprintAgent } from '@/lib/ipc-presets';
import { PROVIDERS, getProvidersByTier } from '@/lib/ipc-providers';
import { CHANNELS } from '@/lib/ipc-channels';
import { generateAgentConfig, generateInstructionsMd, downloadAsFile, type AgentConfigInputs } from '@/lib/ipc-config-gen';
import { createPaircode } from '@/lib/ipc-api';
import TrustBadge from './TrustBadge';

interface Props {
  open: boolean;
  onClose: () => void;
  onCreated: () => void;
  brokerUrl: string;
}

type Step = 'blueprint' | 'provider' | 'channels' | 'review' | 'result';
const STEPS: Step[] = ['blueprint', 'provider', 'channels', 'review', 'result'];

interface AgentRow {
  agent: BlueprintAgent;
  name: string;
  providerId: string;
  apiKey: string;
  model: string;
  baseUrl: string;
  channelId: string;
  channelValues: Record<string, string>;
  gatewayPort: number;
  // result
  pairingCode: string;
}

export default function DeployBlueprintDialog({ open, onClose, onCreated, brokerUrl }: Props) {
  const [step, setStep] = useState<Step>('blueprint');
  const [error, setError] = useState<string | null>(null);

  const [selectedBlueprint, setSelectedBlueprint] = useState<FleetBlueprint | null>(null);
  const [sameProvider, setSameProvider] = useState(true);
  const [sharedProviderId, setSharedProviderId] = useState('anthropic');
  const [sharedApiKey, setSharedApiKey] = useState('');
  const [sharedBaseUrl, setSharedBaseUrl] = useState('');
  const [rows, setRows] = useState<AgentRow[]>([]);

  const [creating, setCreating] = useState(false);
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null);

  if (!open) return null;

  const stepIdx = STEPS.indexOf(step);

  const reset = () => {
    setStep('blueprint');
    setSelectedBlueprint(null);
    setSameProvider(true);
    setSharedProviderId('anthropic');
    setSharedApiKey('');
    setRows([]);
    setError(null);
  };

  const handleClose = () => { reset(); onClose(); };

  const selectBlueprint = (bp: FleetBlueprint) => {
    setSelectedBlueprint(bp);
    const newRows: AgentRow[] = bp.agents.map((a, i) => {
      const preset = getPresetById(a.preset_id);
      return {
        agent: a,
        name: a.default_name,
        providerId: preset?.suggested_provider ?? 'anthropic',
        apiKey: '',
        model: preset?.suggested_model ?? 'claude-sonnet-4-6',
        baseUrl: '',
        channelId: a.suggested_channel ?? 'none',
        channelValues: {},
        gatewayPort: 42618 + i,
        pairingCode: '',
      };
    });
    setRows(newRows);
    setStep('provider');
  };

  const updateRow = (idx: number, patch: Partial<AgentRow>) => {
    setRows((prev) => prev.map((r, i) => i === idx ? { ...r, ...patch } : r));
  };

  const handleCreate = async () => {
    if (!selectedBlueprint) return;
    setCreating(true);
    setError(null);
    try {
      const updated = [...rows];
      for (let i = 0; i < updated.length; i++) {
        const row = updated[i]!;
        const preset = getPresetById(row.agent.preset_id);
        const result = await createPaircode(row.name, preset?.trust_level ?? 3, preset?.role ?? 'agent');
        updated[i] = { ...row, pairingCode: result.pairing_code };
      }
      setRows(updated);
      setStep('result');
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to create pairing codes');
    } finally {
      setCreating(false);
    }
  };

  const canNext = (): boolean => {
    switch (step) {
      case 'provider': {
        if (sameProvider) {
          const p = PROVIDERS.find((pr) => pr.id === sharedProviderId);
          if (p?.credential_type === 'api_key' && !sharedApiKey) return false;
          if ((sharedProviderId === 'custom' || p?.tier === 'local') && !sharedBaseUrl) return false;
        } else {
          for (const row of rows) {
            const p = PROVIDERS.find((pr) => pr.id === row.providerId);
            if (p?.credential_type === 'api_key' && !row.apiKey) return false;
            if ((row.providerId === 'custom' || p?.tier === 'local') && !row.baseUrl) return false;
          }
        }
        return rows.every((r) => r.model.trim().length > 0);
      }
      case 'channels': {
        for (const row of rows) {
          if (row.channelId === 'none' || !row.channelId) continue;
          const ch = CHANNELS.find((c) => c.id === row.channelId);
          if (!ch) continue;
          const missing = ch.fields.filter((f) => f.required).some((f) => !row.channelValues[f.key]?.trim());
          if (missing) return false;
        }
        return true;
      }
      default: return true;
    }
  };

  const goNext = () => {
    const idx = STEPS.indexOf(step);
    if (step === 'review') {
      handleCreate();
    } else if (idx < STEPS.length - 1) {
      const next = STEPS[idx + 1];
      if (next) setStep(next);
    }
  };

  const goBack = () => {
    const idx = STEPS.indexOf(step);
    if (idx > 0) {
      const prev = STEPS[idx - 1];
      if (prev) setStep(prev);
    }
  };

  const buildConfig = (row: AgentRow): string => {
    const preset = getPresetById(row.agent.preset_id);
    const pid = sameProvider ? sharedProviderId : row.providerId;
    const key = sameProvider ? sharedApiKey : row.apiKey;
    const url = sameProvider ? sharedBaseUrl : row.baseUrl;
    const inputs: AgentConfigInputs = {
      agentId: row.name,
      role: preset?.role ?? 'agent',
      trustLevel: preset?.trust_level ?? 3,
      providerId: pid,
      apiKey: key,
      model: row.model,
      baseUrl: url,
      channelId: row.channelId,
      channelValues: row.channelValues,
      brokerUrl,
      gatewayPort: row.gatewayPort,
      systemPrompt: preset?.system_prompt ?? '',
    };
    return generateAgentConfig(inputs);
  };

  const downloadAll = () => {
    rows.forEach((row) => {
      downloadAsFile(`${row.name}-config.toml`, buildConfig(row));
      const preset = getPresetById(row.agent.preset_id);
      if (preset?.system_prompt) {
        downloadAsFile(`${row.name}-instructions.md`, generateInstructionsMd(preset.system_prompt));
      }
    });
  };

  const copyCode = (idx: number, code: string) => {
    navigator.clipboard.writeText(code);
    setCopiedIdx(idx);
    setTimeout(() => setCopiedIdx(null), 2000);
  };

  return createPortal(
    <div className="fixed inset-0 z-[9999] flex items-center justify-center">
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={handleClose} />
      <div className="relative w-full max-w-3xl max-h-[85vh] overflow-auto glass-card p-6 animate-fade-in-scale">
        <div className="flex justify-between items-center mb-6">
          <div>
            <h2 className="text-xl font-bold text-white">Deploy Blueprint</h2>
            <div className="flex gap-1 mt-2">
              {STEPS.map((s, i) => (
                <div key={s} className={`h-1 flex-1 rounded-full ${i <= stepIdx ? 'bg-[#0080ff]' : 'bg-[#1a1a3e]'}`} />
              ))}
            </div>
          </div>
          <button onClick={handleClose} className="text-[#556080] hover:text-white text-xl">&times;</button>
        </div>

        {error && (
          <div className="mb-4 p-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm">{error}</div>
        )}

        {/* Step: Blueprint */}
        {step === 'blueprint' && (
          <div className="space-y-3">
            <p className="text-sm text-[#556080]">Choose a fleet blueprint</p>
            {FLEET_BLUEPRINTS.map((bp) => (
              <button
                key={bp.id}
                onClick={() => selectBlueprint(bp)}
                className="w-full text-left p-4 rounded-xl border border-[#1a1a3e]/50 hover:border-[#0080ff40] hover:bg-[#0080ff05] transition-all"
              >
                <div className="flex items-center gap-3">
                  <span className="text-2xl">{bp.icon}</span>
                  <div className="flex-1">
                    <div className="flex items-center gap-2">
                      <span className="font-medium text-white">{bp.name}</span>
                      <span className="text-xs text-[#556080]">{bp.agents.length} agents</span>
                    </div>
                    <p className="text-xs text-[#556080] mt-0.5">{bp.description}</p>
                    <div className="flex gap-1 mt-1.5 flex-wrap">
                      {bp.agents.map((a) => {
                        const preset = getPresetById(a.preset_id);
                        return (
                          <span key={a.default_name} className="text-[10px] px-1.5 py-0.5 rounded bg-[#1a1a3e]/50 text-[#8892a8]">
                            {a.default_name} <TrustBadge level={preset?.trust_level ?? 3} />
                          </span>
                        );
                      })}
                    </div>
                  </div>
                </div>
              </button>
            ))}
          </div>
        )}

        {/* Step: Provider */}
        {step === 'provider' && (
          <div className="space-y-4">
            <label className="flex items-center gap-2 text-sm text-[#8892a8] cursor-pointer">
              <input type="checkbox" checked={sameProvider} onChange={(e) => setSameProvider(e.target.checked)} className="accent-[#0080ff]" />
              Same provider for all agents
            </label>

            {sameProvider ? (
              <div className="space-y-3">
                <ProviderSelect value={sharedProviderId} onChange={setSharedProviderId} />
                {(PROVIDERS.find((p) => p.id === sharedProviderId)?.credential_type === 'api_key' || sharedProviderId === 'custom') && (
                  <Field label="API Key">
                    <input type="password" value={sharedApiKey} onChange={(e) => setSharedApiKey(e.target.value)} placeholder="sk-..." className="input-electric px-3 py-2 text-sm w-full" />
                  </Field>
                )}
                {(sharedProviderId === 'custom' || PROVIDERS.find((p) => p.id === sharedProviderId)?.tier === 'local') && (
                  <Field label="Base URL">
                    <input type="text" value={sharedBaseUrl} onChange={(e) => setSharedBaseUrl(e.target.value)} placeholder="http://localhost:11434/v1" className="input-electric px-3 py-2 text-sm w-full" />
                  </Field>
                )}
                <p className="text-xs text-[#334060]">Per-agent model overrides:</p>
                {rows.map((row, i) => (
                  <div key={row.name} className="flex items-center gap-2">
                    <span className="text-xs text-[#556080] w-32 truncate">{row.name}</span>
                    <input
                      type="text"
                      value={row.model}
                      onChange={(e) => updateRow(i, { model: e.target.value })}
                      className="input-electric px-2 py-1 text-xs flex-1"
                    />
                  </div>
                ))}
              </div>
            ) : (
              <div className="space-y-4">
                {rows.map((row, i) => (
                  <div key={row.name} className="p-3 rounded-lg border border-[#1a1a3e]/30 space-y-2">
                    <span className="text-sm font-medium text-white">{row.name}</span>
                    <ProviderSelect value={row.providerId} onChange={(v) => {
                      const p = PROVIDERS.find((pr) => pr.id === v);
                      updateRow(i, { providerId: v, model: p?.default_model ?? row.model });
                    }} />
                    {(PROVIDERS.find((p) => p.id === row.providerId)?.credential_type === 'api_key' || row.providerId === 'custom') && (
                      <input type="password" value={row.apiKey} onChange={(e) => updateRow(i, { apiKey: e.target.value })} placeholder="API key" className="input-electric px-2 py-1 text-xs w-full" />
                    )}
                    {(row.providerId === 'custom' || PROVIDERS.find((p) => p.id === row.providerId)?.tier === 'local') && (
                      <input type="text" value={row.baseUrl} onChange={(e) => updateRow(i, { baseUrl: e.target.value })} placeholder="http://localhost:11434/v1" className="input-electric px-2 py-1 text-xs w-full" />
                    )}
                    <input type="text" value={row.model} onChange={(e) => updateRow(i, { model: e.target.value })} className="input-electric px-2 py-1 text-xs w-full" placeholder="Model" />
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        {/* Step: Channels */}
        {step === 'channels' && (
          <div className="space-y-3">
            <p className="text-sm text-[#556080]">Assign channels per agent (optional)</p>
            {rows.map((row, i) => {
              const selectedCh = CHANNELS.find((c) => c.id === row.channelId);
              return (
                <div key={row.name} className="p-3 rounded-lg border border-[#1a1a3e]/30 space-y-2">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-white">{row.name}</span>
                    {row.agent.channel_note && <span className="text-[10px] text-[#556080]">({row.agent.channel_note})</span>}
                  </div>
                  <select
                    value={row.channelId}
                    onChange={(e) => updateRow(i, { channelId: e.target.value, channelValues: {} })}
                    className="input-electric px-2 py-1 text-xs w-full"
                  >
                    <option value="none">None — IPC only</option>
                    {CHANNELS.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
                  </select>
                  {selectedCh?.fields.map((field) => (
                    <input
                      key={field.key}
                      type={field.type === 'password' ? 'password' : 'text'}
                      value={row.channelValues[field.key] ?? ''}
                      onChange={(e) => updateRow(i, { channelValues: { ...row.channelValues, [field.key]: e.target.value } })}
                      placeholder={field.label}
                      className="input-electric px-2 py-1 text-xs w-full"
                    />
                  ))}
                </div>
              );
            })}
          </div>
        )}

        {/* Step: Review */}
        {step === 'review' && selectedBlueprint && (
          <div className="space-y-4">
            <p className="text-sm text-[#556080]">Review before creating</p>
            <div className="overflow-x-auto">
              <table className="w-full text-xs">
                <thead>
                  <tr className="text-[#556080] border-b border-[#1a1a3e]/30">
                    <th className="text-left px-2 py-1">Agent</th>
                    <th className="text-left px-2 py-1">Trust</th>
                    <th className="text-left px-2 py-1">Provider</th>
                    <th className="text-left px-2 py-1">Model</th>
                    <th className="text-left px-2 py-1">Channel</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((row) => {
                    const preset = getPresetById(row.agent.preset_id);
                    return (
                      <tr key={row.name} className="border-b border-[#1a1a3e]/20">
                        <td className="px-2 py-1.5 text-white font-mono">{row.name}</td>
                        <td className="px-2 py-1.5"><TrustBadge level={preset?.trust_level ?? 3} /></td>
                        <td className="px-2 py-1.5 text-[#8892a8]">{sameProvider ? sharedProviderId : row.providerId}</td>
                        <td className="px-2 py-1.5 text-[#8892a8]">{row.model}</td>
                        <td className="px-2 py-1.5 text-[#8892a8]">{row.channelId === 'none' ? '—' : row.channelId}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>

            {/* Broker patch */}
            <div className="space-y-1">
              <p className="text-xs text-yellow-400 font-medium">Broker config patch (add to broker's config.toml):</p>
              <pre className="text-xs text-[#8892a8] bg-[#050510] rounded p-3 whitespace-pre-wrap">{selectedBlueprint.broker_patch_toml}</pre>
              <button
                onClick={() => navigator.clipboard.writeText(selectedBlueprint.broker_patch_toml)}
                className="text-[10px] text-[#0080ff] hover:underline"
              >
                Copy patch
              </button>
            </div>
          </div>
        )}

        {/* Step: Result */}
        {step === 'result' && (
          <div className="space-y-4">
            <div className="overflow-x-auto">
              <table className="w-full text-xs">
                <thead>
                  <tr className="text-[#556080] border-b border-[#1a1a3e]/30">
                    <th className="text-left px-2 py-1">Agent</th>
                    <th className="text-left px-2 py-1">Pairing Code</th>
                    <th className="text-right px-2 py-1">Config</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((row, i) => (
                    <tr key={row.name} className="border-b border-[#1a1a3e]/20">
                      <td className="px-2 py-2 text-white font-mono">{row.name}</td>
                      <td className="px-2 py-2">
                        <span className="font-mono text-lg text-[#0080ff] tracking-wider">{row.pairingCode}</span>
                        <button
                          onClick={() => copyCode(i, row.pairingCode)}
                          className="ml-2 text-[10px] text-[#556080] hover:text-white"
                        >
                          {copiedIdx === i ? '✓' : 'copy'}
                        </button>
                      </td>
                      <td className="px-2 py-2 text-right space-x-2">
                        <button
                          onClick={() => downloadAsFile(`${row.name}-config.toml`, buildConfig(row))}
                          className="text-[10px] text-[#0080ff] hover:underline"
                        >
                          config
                        </button>
                        {getPresetById(row.agent.preset_id)?.system_prompt && (
                          <button
                            onClick={() => {
                              const p = getPresetById(row.agent.preset_id);
                              if (p?.system_prompt) downloadAsFile(`${row.name}-instructions.md`, generateInstructionsMd(p.system_prompt));
                            }}
                            className="text-[10px] text-[#556080] hover:underline"
                          >
                            instructions
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            <button onClick={downloadAll} className="btn-electric w-full py-2.5 text-sm font-medium">
              Download All Files
            </button>

            <div className="p-4 rounded-xl bg-[#050510] border border-[#1a1a3e]/50 text-xs text-[#556080] space-y-2">
              <p className="font-medium text-[#8892a8]">Setup instructions:</p>
              <p>1. Place each agent's config.toml in <code className="text-[#0080ff]">~/.zeroclaw/</code> and rename <code className="text-[#0080ff]">&lt;agent&gt;-instructions.md</code> to <code className="text-[#0080ff]">~/.zeroclaw/workspace/instructions.md</code></p>
              <p>2. For each agent, pair with broker:</p>
              <pre className="text-[#0080ff] bg-[#0a0a18] rounded p-2 overflow-x-auto">curl -X POST {brokerUrl}/pair -H &apos;X-Pairing-Code: CODE&apos;</pre>
              <p>3. Save the returned token as <code className="text-[#0080ff]">broker_token</code> in each config.toml under [agents_ipc]</p>
              <p>4. Start each agent: <code className="text-[#0080ff]">zeroclaw daemon</code></p>
              <p>5. <span className="text-yellow-400">Add the broker config patch</span> to your broker's config.toml and restart it</p>
            </div>

            <button
              onClick={() => { handleClose(); onCreated(); }}
              className="w-full py-2.5 text-sm font-medium text-[#8892a8] rounded-lg border border-[#1a1a3e]/50 hover:bg-[#1a1a3e]/30 transition-colors"
            >
              Done
            </button>
          </div>
        )}

        {/* Navigation */}
        {step !== 'result' && step !== 'blueprint' && (
          <div className="flex justify-between mt-6 pt-4 border-t border-[#1a1a3e]/30">
            <button onClick={goBack} className="text-sm text-[#556080] hover:text-white transition-colors">
              &larr; Back
            </button>
            <button
              onClick={goNext}
              disabled={creating || !canNext()}
              className="btn-electric px-6 py-2 text-sm font-medium disabled:opacity-50"
            >
              {creating ? 'Creating...' : step === 'review' ? `Create ${rows.length} Agents` : 'Next →'}
            </button>
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
}

function ProviderSelect({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)} className="input-electric px-2 py-1 text-xs w-full">
      <optgroup label="Recommended">
        {getProvidersByTier('recommended').map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
      </optgroup>
      <optgroup label="Local">
        {getProvidersByTier('local').map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
      </optgroup>
      <option value="custom">Custom — any OpenAI-compatible API</option>
    </select>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="space-y-1">
      <label className="text-xs text-[#556080] uppercase tracking-wider">{label}</label>
      {children}
    </div>
  );
}
