// Provider catalog for Phase 3.6 Agent Provisioning UI
// Mirrors src/onboard/wizard.rs tiers. v1 scope: Tier 1 + Tier 5 + Custom.

export type CredentialType = 'api_key' | 'oauth' | 'none';

export interface ProviderDef {
  id: string;
  name: string;
  tier: 'recommended' | 'local' | 'custom';
  credential_type: CredentialType;
  env_var?: string;
  default_model: string;
  default_base_url?: string;
  description: string;
}

export const PROVIDERS: ProviderDef[] = [
  // ── Tier 1: Recommended ──
  {
    id: 'anthropic',
    name: 'Anthropic',
    tier: 'recommended',
    credential_type: 'api_key',
    env_var: 'ANTHROPIC_API_KEY',
    default_model: 'claude-sonnet-4-6',
    description: 'Claude Sonnet & Opus (direct)',
  },
  {
    id: 'openai',
    name: 'OpenAI',
    tier: 'recommended',
    credential_type: 'api_key',
    env_var: 'OPENAI_API_KEY',
    default_model: 'gpt-4o',
    description: 'GPT-4o, o1, GPT-5 (direct)',
  },
  {
    id: 'openrouter',
    name: 'OpenRouter',
    tier: 'recommended',
    credential_type: 'api_key',
    env_var: 'OPENROUTER_API_KEY',
    default_model: 'anthropic/claude-sonnet-4-6',
    description: '200+ models, 1 API key',
  },
  {
    id: 'deepseek',
    name: 'DeepSeek',
    tier: 'recommended',
    credential_type: 'api_key',
    env_var: 'DEEPSEEK_API_KEY',
    default_model: 'deepseek-chat',
    description: 'V3 & R1 (affordable)',
  },
  {
    id: 'mistral',
    name: 'Mistral',
    tier: 'recommended',
    credential_type: 'api_key',
    env_var: 'MISTRAL_API_KEY',
    default_model: 'mistral-large-latest',
    description: 'Large & Codestral',
  },
  {
    id: 'xai',
    name: 'xAI',
    tier: 'recommended',
    credential_type: 'api_key',
    env_var: 'XAI_API_KEY',
    default_model: 'grok-3',
    description: 'Grok 3 & 4',
  },
  {
    id: 'gemini',
    name: 'Google Gemini',
    tier: 'recommended',
    credential_type: 'api_key',
    env_var: 'GEMINI_API_KEY',
    default_model: 'gemini-2.0-flash',
    description: 'Gemini Flash & Pro',
  },
  {
    id: 'groq',
    name: 'Groq',
    tier: 'recommended',
    credential_type: 'api_key',
    env_var: 'GROQ_API_KEY',
    default_model: 'llama-3.3-70b-versatile',
    description: 'Ultra-fast LPU inference',
  },
  {
    id: 'perplexity',
    name: 'Perplexity',
    tier: 'recommended',
    credential_type: 'api_key',
    env_var: 'PERPLEXITY_API_KEY',
    default_model: 'sonar',
    description: 'Search-augmented AI',
  },
  {
    id: 'venice',
    name: 'Venice AI',
    tier: 'recommended',
    credential_type: 'api_key',
    env_var: 'VENICE_API_KEY',
    default_model: 'llama-3.3-70b',
    description: 'Privacy-first (Llama, Opus)',
  },

  // ── Tier 5: Local / Private ──
  {
    id: 'ollama',
    name: 'Ollama',
    tier: 'local',
    credential_type: 'none',
    default_model: 'llama3.2',
    default_base_url: 'http://localhost:11434',
    description: 'Local models (Llama, Mistral, Phi)',
  },
  {
    id: 'llamacpp',
    name: 'llama.cpp server',
    tier: 'local',
    credential_type: 'none',
    default_model: 'default',
    default_base_url: 'http://localhost:8080/v1',
    description: 'Local OpenAI-compatible endpoint',
  },
  {
    id: 'vllm',
    name: 'vLLM',
    tier: 'local',
    credential_type: 'none',
    default_model: 'default',
    default_base_url: 'http://localhost:8000/v1',
    description: 'High-performance local inference',
  },
];

export function getProvidersByTier(tier: ProviderDef['tier']): ProviderDef[] {
  return PROVIDERS.filter((p) => p.tier === tier);
}

export function getProviderById(id: string): ProviderDef | undefined {
  return PROVIDERS.find((p) => p.id === id);
}
