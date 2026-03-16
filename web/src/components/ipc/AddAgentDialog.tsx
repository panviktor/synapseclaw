import { useState } from 'react';
import { createPortal } from 'react-dom';
import { AGENT_PRESETS, type AgentPreset } from '@/lib/ipc-presets';
import { PROVIDERS, getProvidersByTier } from '@/lib/ipc-providers';
import { CHANNELS } from '@/lib/ipc-channels';
import { generateAgentConfig, downloadAsFile, type AgentConfigInputs } from '@/lib/ipc-config-gen';
import { createPaircode } from '@/lib/ipc-api';
import TrustBadge from './TrustBadge';

interface Props {
  open: boolean;
  onClose: () => void;
  onCreated: () => void;
  brokerUrl: string;
}

type Step = 'preset' | 'identity' | 'provider' | 'channel' | 'result';
const STEPS: Step[] = ['preset', 'identity', 'provider', 'channel', 'result'];

export default function AddAgentDialog({ open, onClose, onCreated, brokerUrl }: Props) {
  const [step, setStep] = useState<Step>('preset');
  const [error, setError] = useState<string | null>(null);

  // Form state
  const [selectedPreset, setSelectedPreset] = useState<AgentPreset | null>(null);
  const [agentId, setAgentId] = useState('');
  const [role, setRole] = useState('');
  const [trustLevel, setTrustLevel] = useState(3);
  const [providerId, setProviderId] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [model, setModel] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [channelId, setChannelId] = useState('none');
  const [channelValues, setChannelValues] = useState<Record<string, string>>({});
  const [gatewayPort, setGatewayPort] = useState(42618);
  const [systemPrompt, setSystemPrompt] = useState('');

  // Result
  const [pairingCode, setPairingCode] = useState('');
  const [configToml, setConfigToml] = useState('');
  const [creating, setCreating] = useState(false);
  const [copied, setCopied] = useState(false);

  if (!open) return null;

  const stepIdx = STEPS.indexOf(step);

  const reset = () => {
    setStep('preset');
    setSelectedPreset(null);
    setAgentId('');
    setRole('');
    setTrustLevel(3);
    setProviderId('');
    setApiKey('');
    setModel('');
    setBaseUrl('');
    setChannelId('none');
    setChannelValues({});
    setGatewayPort(42618);
    setSystemPrompt('');
    setPairingCode('');
    setConfigToml('');
    setError(null);
    setCopied(false);
  };

  const handleClose = () => {
    reset();
    onClose();
  };

  const selectPreset = (preset: AgentPreset) => {
    setSelectedPreset(preset);
    setAgentId('');
    setRole(preset.role);
    setTrustLevel(preset.trust_level);
    setProviderId(preset.suggested_provider);
    setModel(preset.suggested_model);
    setSystemPrompt(preset.system_prompt);
    setStep('identity');
  };

  const handleCreate = async () => {
    setCreating(true);
    setError(null);
    try {
      const result = await createPaircode(agentId, trustLevel, role);
      setPairingCode(result.pairing_code);

      const inputs: AgentConfigInputs = {
        agentId, role, trustLevel, providerId, apiKey, model, baseUrl,
        channelId, channelValues, brokerUrl, gatewayPort, systemPrompt,
      };
      const toml = generateAgentConfig(inputs);
      setConfigToml(toml);
      setStep('result');
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to create pairing code');
    } finally {
      setCreating(false);
    }
  };

  const goNext = () => {
    const idx = STEPS.indexOf(step);
    if (step === 'channel') {
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

  const canNext = (): boolean => {
    switch (step) {
      case 'preset': return selectedPreset !== null;
      case 'identity': return agentId.length > 0 && /^[a-z0-9_-]+$/.test(agentId);
      case 'provider': {
        if (!providerId || !model) return false;
        const p = PROVIDERS.find((pr) => pr.id === providerId);
        if (p?.credential_type === 'api_key' && !apiKey) return false;
        return true;
      }
      case 'channel': {
        if (channelId === 'none' || !channelId) return true;
        const ch = CHANNELS.find((c) => c.id === channelId);
        if (!ch) return true;
        return ch.fields.filter((f) => f.required).every((f) => channelValues[f.key]?.trim());
      }
      default: return false;
    }
  };

  const selectedChannel = CHANNELS.find((c) => c.id === channelId);

  return createPortal(
    <div className="fixed inset-0 z-[9999] flex items-center justify-center">
      <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={handleClose} />
      <div className="relative w-full max-w-2xl max-h-[85vh] overflow-auto glass-card p-6 animate-fade-in-scale">
        {/* Header */}
        <div className="flex justify-between items-center mb-6">
          <div>
            <h2 className="text-xl font-bold text-white">Add Agent</h2>
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

        {/* Step: Preset */}
        {step === 'preset' && (
          <div className="space-y-3">
            <p className="text-sm text-[#556080]">Choose an agent template</p>
            <div className="grid grid-cols-1 gap-2">
              {AGENT_PRESETS.map((preset) => (
                <button
                  key={preset.id}
                  onClick={() => selectPreset(preset)}
                  className={`text-left p-4 rounded-xl border transition-all ${
                    selectedPreset?.id === preset.id
                      ? 'border-[#0080ff] bg-[#0080ff10]'
                      : 'border-[#1a1a3e]/50 hover:border-[#0080ff40] hover:bg-[#0080ff05]'
                  }`}
                >
                  <div className="flex items-center gap-3">
                    <span className="text-2xl">{preset.icon}</span>
                    <div className="flex-1">
                      <div className="flex items-center gap-2">
                        <span className="font-medium text-white">{preset.name}</span>
                        <TrustBadge level={preset.trust_level} />
                      </div>
                      <p className="text-xs text-[#556080] mt-0.5">{preset.description}</p>
                    </div>
                  </div>
                </button>
              ))}
            </div>
          </div>
        )}

        {/* Step: Identity */}
        {step === 'identity' && (
          <div className="space-y-4">
            <p className="text-sm text-[#556080]">Agent identity in the IPC fleet</p>
            <Field label="Agent ID" required>
              <input
                type="text"
                value={agentId}
                onChange={(e) => setAgentId(e.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, ''))}
                placeholder="e.g. research, kids-pi"
                className="input-electric px-3 py-2 text-sm w-full"
              />
              {agentId && !/^[a-z0-9_-]+$/.test(agentId) && (
                <p className="text-xs text-red-400 mt-1">Lowercase letters, numbers, dashes, underscores only</p>
              )}
            </Field>
            <Field label="Role">
              <input
                type="text"
                value={role}
                onChange={(e) => setRole(e.target.value)}
                className="input-electric px-3 py-2 text-sm w-full"
              />
            </Field>
            <Field label="Trust Level">
              <div className="flex items-center gap-3">
                <select
                  value={trustLevel}
                  onChange={(e) => setTrustLevel(Number(e.target.value))}
                  className="input-electric px-3 py-2 text-sm"
                >
                  <option value={0}>L0 — Admin</option>
                  <option value={1}>L1 — Coordinator</option>
                  <option value={2}>L2 — Privileged</option>
                  <option value={3}>L3 — Worker</option>
                  <option value={4}>L4 — Restricted</option>
                </select>
                <TrustBadge level={trustLevel} />
                {selectedPreset && trustLevel !== selectedPreset.trust_level && (
                  <span className="text-xs text-yellow-400">Changed from preset default (L{selectedPreset.trust_level})</span>
                )}
              </div>
            </Field>
          </div>
        )}

        {/* Step: Provider */}
        {step === 'provider' && (
          <div className="space-y-4">
            <p className="text-sm text-[#556080]">AI model provider — API key stays local, never sent to broker</p>
            <Field label="Provider">
              <select
                value={providerId}
                onChange={(e) => {
                  setProviderId(e.target.value);
                  const p = PROVIDERS.find((pr) => pr.id === e.target.value);
                  if (p) {
                    setModel(p.default_model);
                    setBaseUrl(p.default_base_url ?? '');
                  }
                }}
                className="input-electric px-3 py-2 text-sm w-full"
              >
                <option value="">Select provider...</option>
                <optgroup label="Recommended">
                  {getProvidersByTier('recommended').map((p) => (
                    <option key={p.id} value={p.id}>{p.name} — {p.description}</option>
                  ))}
                </optgroup>
                <optgroup label="Local / Private">
                  {getProvidersByTier('local').map((p) => (
                    <option key={p.id} value={p.id}>{p.name} — {p.description}</option>
                  ))}
                </optgroup>
                <option value="custom">Custom — any OpenAI-compatible API</option>
              </select>
            </Field>
            {providerId && PROVIDERS.find((p) => p.id === providerId)?.credential_type === 'api_key' && (
              <Field label="API Key">
                <input
                  type="password"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder="sk-..."
                  className="input-electric px-3 py-2 text-sm w-full"
                />
                <p className="text-xs text-[#334060] mt-1">Stored in config.toml only — never sent to broker</p>
              </Field>
            )}
            <Field label="Model">
              <input
                type="text"
                value={model}
                onChange={(e) => setModel(e.target.value)}
                className="input-electric px-3 py-2 text-sm w-full"
              />
            </Field>
            {(providerId === 'custom' || PROVIDERS.find((p) => p.id === providerId)?.tier === 'local') && (
              <Field label="Base URL">
                <input
                  type="text"
                  value={baseUrl}
                  onChange={(e) => setBaseUrl(e.target.value)}
                  placeholder="http://localhost:11434"
                  className="input-electric px-3 py-2 text-sm w-full"
                />
              </Field>
            )}
          </div>
        )}

        {/* Step: Channel */}
        {step === 'channel' && (
          <div className="space-y-4">
            <p className="text-sm text-[#556080]">Optional: messaging channel for this agent</p>
            <Field label="Channel">
              <select
                value={channelId}
                onChange={(e) => { setChannelId(e.target.value); setChannelValues({}); }}
                className="input-electric px-3 py-2 text-sm w-full"
              >
                <option value="none">None — IPC only</option>
                <optgroup label="Messaging">
                  {CHANNELS.filter((c) => c.category === 'messaging').map((c) => (
                    <option key={c.id} value={c.id}>{c.name}{c.feature_gate ? ` (requires --features ${c.feature_gate})` : ''}</option>
                  ))}
                </optgroup>
                <optgroup label="Work / Enterprise">
                  {CHANNELS.filter((c) => c.category === 'work').map((c) => (
                    <option key={c.id} value={c.id}>{c.name}</option>
                  ))}
                </optgroup>
              </select>
            </Field>
            {selectedChannel?.note && (
              <p className="text-xs text-yellow-400">{selectedChannel.note}</p>
            )}
            {selectedChannel?.feature_gate && (
              <p className="text-xs text-yellow-400">Build with: cargo build --features {selectedChannel.feature_gate}</p>
            )}
            {selectedChannel?.fields.map((field) => (
              <Field key={field.key} label={field.label} required={field.required}>
                <input
                  type={field.type === 'password' ? 'password' : 'text'}
                  value={channelValues[field.key] ?? ''}
                  onChange={(e) => setChannelValues({ ...channelValues, [field.key]: e.target.value })}
                  placeholder={field.placeholder}
                  className="input-electric px-3 py-2 text-sm w-full"
                />
                {field.help && <p className="text-xs text-[#334060] mt-1">{field.help}</p>}
              </Field>
            ))}
            <Field label="Gateway Port">
              <input
                type="number"
                value={gatewayPort}
                onChange={(e) => setGatewayPort(Number(e.target.value))}
                className="input-electric px-3 py-2 text-sm w-32"
              />
            </Field>
          </div>
        )}

        {/* Step: Result */}
        {step === 'result' && (
          <div className="space-y-4">
            <div className="p-4 rounded-xl bg-[#0080ff10] border border-[#0080ff30] text-center">
              <p className="text-xs text-[#556080] uppercase tracking-wider mb-2">Pairing Code</p>
              <p className="text-4xl font-mono font-bold text-[#0080ff] tracking-widest">{pairingCode}</p>
              <button
                onClick={() => { navigator.clipboard.writeText(pairingCode); setCopied(true); setTimeout(() => setCopied(false), 2000); }}
                className="mt-2 text-xs text-[#556080] hover:text-white transition-colors"
              >
                {copied ? 'Copied!' : 'Copy to clipboard'}
              </button>
            </div>

            <div className="space-y-2">
              <button
                onClick={() => downloadAsFile(`${agentId}-config.toml`, configToml)}
                className="btn-electric w-full py-2.5 text-sm font-medium"
              >
                Download {agentId}-config.toml
              </button>
            </div>

            <div className="p-4 rounded-xl bg-[#050510] border border-[#1a1a3e]/50 text-xs text-[#556080] space-y-2">
              <p className="font-medium text-[#8892a8]">Setup instructions:</p>
              <p>1. Place config.toml in <code className="text-[#0080ff]">~/.zeroclaw/</code> on the target machine</p>
              <p>2. Pair with broker:</p>
              <pre className="text-[#0080ff] bg-[#0a0a18] rounded p-2 overflow-x-auto">curl -X POST {brokerUrl}/pair -H &apos;X-Pairing-Code: {pairingCode}&apos;</pre>
              <p>3. Save the returned token as <code className="text-[#0080ff]">broker_token</code> in config.toml under [agents_ipc]</p>
              <p>4. Run: <code className="text-[#0080ff]">zeroclaw daemon</code></p>
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
        {step !== 'result' && step !== 'preset' && (
          <div className="flex justify-between mt-6 pt-4 border-t border-[#1a1a3e]/30">
            <button onClick={goBack} className="text-sm text-[#556080] hover:text-white transition-colors">
              &larr; Back
            </button>
            <button
              onClick={goNext}
              disabled={!canNext() || creating}
              className="btn-electric px-6 py-2 text-sm font-medium disabled:opacity-50"
            >
              {creating ? 'Creating...' : step === 'channel' ? 'Create Agent' : 'Next →'}
            </button>
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
}

function Field({ label, required, children }: { label: string; required?: boolean; children: React.ReactNode }) {
  return (
    <div className="space-y-1">
      <label className="text-xs text-[#556080] uppercase tracking-wider">
        {label}{required && <span className="text-red-400 ml-1">*</span>}
      </label>
      {children}
    </div>
  );
}
