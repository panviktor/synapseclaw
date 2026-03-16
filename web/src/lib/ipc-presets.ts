// Agent presets & fleet blueprints for Phase 3.6 Provisioning UI

export interface AgentPreset {
  id: string;
  name: string;
  description: string;
  icon: string;
  trust_level: number;
  role: string;
  suggested_provider: string;
  suggested_model: string;
  tools: string[];
  system_prompt: string;
}

export const AGENT_PRESETS: AgentPreset[] = [
  {
    id: 'coordinator',
    name: 'Coordinator',
    description: 'Main orchestrator — delegates tasks, synthesizes results, makes decisions',
    icon: '👑',
    trust_level: 1,
    role: 'coordinator',
    suggested_provider: 'anthropic',
    suggested_model: 'claude-opus-4-6',
    tools: [],
    system_prompt: 'You are the primary coordinator. Delegate tasks to specialists, synthesize results, make decisions.',
  },
  {
    id: 'ops',
    name: 'Ops Monitor',
    description: 'Infrastructure monitoring — diagnostics, incident response, health checks',
    icon: '🛡️',
    trust_level: 2,
    role: 'monitor',
    suggested_provider: 'anthropic',
    suggested_model: 'claude-sonnet-4-6',
    tools: ['shell', 'http_request', 'memory_read', 'memory_write', 'agents_send', 'agents_inbox'],
    system_prompt: 'You monitor infrastructure health. Run diagnostics, report incidents upstream, escalate destructive actions.',
  },
  {
    id: 'research',
    name: 'Research Worker',
    description: 'Information gathering — web search, browsing, analysis',
    icon: '🔬',
    trust_level: 3,
    role: 'researcher',
    suggested_provider: 'anthropic',
    suggested_model: 'claude-sonnet-4-6',
    tools: ['web_search', 'web_fetch', 'memory_read', 'memory_write', 'agents_send', 'agents_inbox', 'agents_reply'],
    system_prompt: 'You research topics using web search and browsing. Return structured findings to the coordinator.',
  },
  {
    id: 'code',
    name: 'Code Worker',
    description: 'Development — code review, testing, file operations',
    icon: '💻',
    trust_level: 3,
    role: 'developer',
    suggested_provider: 'anthropic',
    suggested_model: 'claude-sonnet-4-6',
    tools: ['shell', 'file_read', 'file_write', 'memory_read', 'memory_write', 'agents_send', 'agents_inbox', 'agents_reply'],
    system_prompt: 'You write, review, and test code. Work within the workspace. Report results upstream.',
  },
  {
    id: 'restricted',
    name: 'Restricted Assistant',
    description: 'Low-trust environment — children, guests, minimal permissions',
    icon: '🔒',
    trust_level: 4,
    role: 'restricted',
    suggested_provider: 'anthropic',
    suggested_model: 'claude-haiku-4-5',
    tools: ['memory_read', 'web_search'],
    system_prompt: 'You are a friendly assistant. Answer questions, help with homework, tell stories. No commands, no files.',
  },
];

export interface BlueprintAgent {
  preset_id: string;
  default_name: string;
  suggested_channel?: string;
  channel_note?: string;
}

export interface FleetBlueprint {
  id: string;
  name: string;
  description: string;
  icon: string;
  agents: BlueprintAgent[];
  lateral_text_pairs: [string, string][];
  l4_destinations: Record<string, string>;
  coordinator_prompt_addition: string;
  broker_patch_toml: string;
}

export const FLEET_BLUEPRINTS: FleetBlueprint[] = [
  {
    id: 'marketing',
    name: 'Marketing Pipeline',
    description: 'News monitoring, trend aggregation, copywriting, publishing',
    icon: '📢',
    agents: [
      { preset_id: 'coordinator', default_name: 'marketing-lead', suggested_channel: 'telegram', channel_note: 'Operator notifications' },
      { preset_id: 'research', default_name: 'news-reader' },
      { preset_id: 'research', default_name: 'trend-aggregator' },
      { preset_id: 'code', default_name: 'copywriter' },
      { preset_id: 'ops', default_name: 'publisher' },
    ],
    lateral_text_pairs: [['news-reader', 'trend-aggregator']],
    l4_destinations: {},
    coordinator_prompt_addition: 'You manage a content pipeline. Delegate news gathering to news-reader, trend analysis to trend-aggregator, copywriting to copywriter, and publishing to publisher. Review all content before publishing.',
    broker_patch_toml: `[agents_ipc]
lateral_text_pairs = [["news-reader", "trend-aggregator"]]`,
  },
  {
    id: 'office',
    name: 'Office Assistant',
    description: 'Email management, calendar reminders, document drafting',
    icon: '📋',
    agents: [
      { preset_id: 'coordinator', default_name: 'office-lead', suggested_channel: 'telegram', channel_note: 'Or Slack' },
      { preset_id: 'research', default_name: 'email-watcher' },
      { preset_id: 'research', default_name: 'calendar-bot' },
      { preset_id: 'code', default_name: 'doc-writer' },
    ],
    lateral_text_pairs: [],
    l4_destinations: {},
    coordinator_prompt_addition: 'You manage an office assistant team. email-watcher reads incoming mail and escalates. calendar-bot monitors events. doc-writer drafts documents. You decide and delegate.',
    broker_patch_toml: `[agents_ipc]
lateral_text_pairs = []`,
  },
  {
    id: 'devteam',
    name: 'Dev Team',
    description: 'Code review, testing, deployment monitoring',
    icon: '⚙️',
    agents: [
      { preset_id: 'coordinator', default_name: 'dev-lead', suggested_channel: 'slack', channel_note: 'Or Discord' },
      { preset_id: 'code', default_name: 'reviewer' },
      { preset_id: 'code', default_name: 'test-runner' },
      { preset_id: 'ops', default_name: 'ops' },
    ],
    lateral_text_pairs: [['reviewer', 'test-runner'], ['ops', 'reviewer']],
    l4_destinations: {},
    coordinator_prompt_addition: 'You lead a dev team. reviewer does code reviews, test-runner runs tests, ops monitors deployments. Coordinate PR flow.',
    broker_patch_toml: `[agents_ipc]
lateral_text_pairs = [["reviewer", "test-runner"], ["ops", "reviewer"]]`,
  },
  {
    id: 'family',
    name: 'Family',
    description: 'Home system with restricted access for children',
    icon: '🏠',
    agents: [
      { preset_id: 'coordinator', default_name: 'opus', suggested_channel: 'telegram', channel_note: 'Parent chat' },
      { preset_id: 'research', default_name: 'daily', suggested_channel: 'telegram', channel_note: 'Family group' },
      { preset_id: 'restricted', default_name: 'kids', suggested_channel: 'telegram', channel_note: 'Kids bot' },
      { preset_id: 'restricted', default_name: 'tutor', suggested_channel: 'telegram', channel_note: 'Tutor bot' },
    ],
    lateral_text_pairs: [],
    l4_destinations: { supervisor: 'opus', escalation: 'opus' },
    coordinator_prompt_addition: 'You manage a family assistant network. daily sends morning digests. kids and tutor serve the children — their messages arrive in the quarantine lane. Review quarantine content before acting.',
    broker_patch_toml: `[agents_ipc]
lateral_text_pairs = []

[agents_ipc.l4_destinations]
supervisor = "opus"
escalation = "opus"`,
  },
  {
    id: 'research_bureau',
    name: 'Research Bureau',
    description: 'Deep research with specialized investigators',
    icon: '🔎',
    agents: [
      { preset_id: 'coordinator', default_name: 'research-lead', suggested_channel: 'telegram' },
      { preset_id: 'research', default_name: 'web-researcher' },
      { preset_id: 'research', default_name: 'analyst' },
      { preset_id: 'code', default_name: 'report-writer' },
    ],
    lateral_text_pairs: [['web-researcher', 'analyst']],
    l4_destinations: {},
    coordinator_prompt_addition: 'You lead a research bureau. web-researcher does broad search, analyst does deep analysis, report-writer structures findings. Synthesize final reports.',
    broker_patch_toml: `[agents_ipc]
lateral_text_pairs = [["web-researcher", "analyst"]]`,
  },
];

export function getPresetById(id: string): AgentPreset | undefined {
  return AGENT_PRESETS.find((p) => p.id === id);
}

export function getBlueprintById(id: string): FleetBlueprint | undefined {
  return FLEET_BLUEPRINTS.find((b) => b.id === id);
}
