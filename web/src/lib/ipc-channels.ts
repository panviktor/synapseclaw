// Channel catalog for Phase 3.6 Agent Provisioning UI
// Fields verified against src/config/schema.rs

export type ChannelCategory = 'messaging' | 'work';

export interface ChannelField {
  key: string;
  label: string;
  required: boolean;
  type: 'text' | 'password' | 'list';
  placeholder?: string;
  help?: string;
}

export interface ChannelDef {
  id: string;
  name: string;
  category: ChannelCategory;
  fields: ChannelField[];
  feature_gate?: string;
  note?: string;
}

export const CHANNELS: ChannelDef[] = [
  // ── Messaging ──
  {
    id: 'telegram',
    name: 'Telegram',
    category: 'messaging',
    fields: [
      { key: 'bot_token', label: 'Bot Token', required: true, type: 'password', placeholder: '123456:ABC-DEF...', help: 'From @BotFather' },
      { key: 'allowed_users', label: 'Allowed Users', required: true, type: 'list', placeholder: 'user_id or username, comma-separated' },
    ],
  },
  {
    id: 'discord',
    name: 'Discord',
    category: 'messaging',
    fields: [
      { key: 'bot_token', label: 'Bot Token', required: true, type: 'password', help: 'From Discord Developer Portal' },
      { key: 'allowed_users', label: 'Allowed User IDs', required: true, type: 'list', placeholder: 'Discord user IDs, comma-separated' },
      { key: 'guild_id', label: 'Guild ID', required: false, type: 'text', placeholder: 'Optional: restrict to one server' },
    ],
  },
  {
    id: 'slack',
    name: 'Slack',
    category: 'messaging',
    fields: [
      { key: 'bot_token', label: 'Bot Token (xoxb-...)', required: true, type: 'password' },
      { key: 'allowed_users', label: 'Allowed User IDs', required: true, type: 'list' },
      { key: 'app_token', label: 'App Token (xapp-...)', required: false, type: 'password', help: 'For Socket Mode' },
      { key: 'channel_id', label: 'Channel ID', required: false, type: 'text' },
    ],
  },
  {
    id: 'matrix',
    name: 'Matrix',
    category: 'messaging',
    feature_gate: 'channel-matrix',
    fields: [
      { key: 'homeserver', label: 'Homeserver URL', required: true, type: 'text', placeholder: 'https://matrix.org' },
      { key: 'room_id', label: 'Room ID', required: true, type: 'text', placeholder: '!abc123:matrix.org' },
      { key: 'allowed_users', label: 'Allowed Users', required: true, type: 'list', placeholder: '@user:matrix.org' },
      { key: 'access_token', label: 'Access Token', required: false, type: 'password', help: 'Or use password below' },
      { key: 'user_id', label: 'User ID', required: false, type: 'text', placeholder: '@bot:matrix.org', help: 'Required with password login' },
      { key: 'password', label: 'Password', required: false, type: 'password', help: 'Alternative to access_token' },
    ],
  },
  {
    id: 'mattermost',
    name: 'Mattermost',
    category: 'messaging',
    fields: [
      { key: 'url', label: 'Server URL', required: true, type: 'text', placeholder: 'https://mattermost.example.com' },
      { key: 'bot_token', label: 'Bot Token', required: true, type: 'password' },
      { key: 'allowed_users', label: 'Allowed User IDs', required: true, type: 'list' },
      { key: 'channel_id', label: 'Channel ID', required: false, type: 'text' },
    ],
  },
  {
    id: 'signal',
    name: 'Signal',
    category: 'messaging',
    fields: [
      { key: 'http_url', label: 'signal-cli HTTP URL', required: true, type: 'text', placeholder: 'http://127.0.0.1:8686' },
      { key: 'account', label: 'Phone Number (E.164)', required: true, type: 'text', placeholder: '+1234567890' },
      { key: 'group_id', label: 'Group ID', required: false, type: 'text', help: '"dm" for DMs only, or specific group ID' },
      { key: 'allowed_from', label: 'Allowed Senders', required: false, type: 'list', placeholder: 'E.164 numbers or *' },
    ],
  },
  {
    id: 'whatsapp',
    name: 'WhatsApp',
    category: 'messaging',
    note: 'Cloud API mode. For Web mode, set session_path instead.',
    fields: [
      { key: 'access_token', label: 'Access Token', required: true, type: 'password', help: 'From Meta Business Suite' },
      { key: 'phone_number_id', label: 'Phone Number ID', required: true, type: 'text', help: 'From Meta Business API' },
      { key: 'verify_token', label: 'Webhook Verify Token', required: true, type: 'password', help: 'You define this, Meta sends it back' },
      { key: 'allowed_numbers', label: 'Allowed Numbers', required: true, type: 'list', placeholder: 'E.164 format or *' },
      { key: 'app_secret', label: 'App Secret', required: false, type: 'password', help: 'For webhook signature verification' },
    ],
  },
  {
    id: 'imessage',
    name: 'iMessage',
    category: 'messaging',
    note: 'macOS only',
    fields: [
      { key: 'allowed_contacts', label: 'Allowed Contacts', required: true, type: 'list', placeholder: 'Phone numbers or emails' },
    ],
  },
  {
    id: 'irc',
    name: 'IRC',
    category: 'messaging',
    fields: [
      { key: 'server', label: 'Server', required: true, type: 'text', placeholder: 'irc.libera.chat' },
      { key: 'nickname', label: 'Nickname', required: true, type: 'text' },
      { key: 'channels', label: 'Channels', required: true, type: 'list', placeholder: '#channel1, #channel2' },
      { key: 'port', label: 'Port', required: false, type: 'text', placeholder: '6697' },
      { key: 'server_password', label: 'Server Password', required: false, type: 'password', help: 'For bouncers like ZNC' },
      { key: 'nickserv_password', label: 'NickServ Password', required: false, type: 'password' },
      { key: 'sasl_password', label: 'SASL Password', required: false, type: 'password', help: 'IRCv3 SASL PLAIN' },
    ],
  },

  // ── Work / Enterprise ──
  {
    id: 'lark',
    name: 'Lark / Feishu',
    category: 'work',
    fields: [
      { key: 'app_id', label: 'App ID', required: true, type: 'text' },
      { key: 'app_secret', label: 'App Secret', required: true, type: 'password' },
    ],
  },
  {
    id: 'dingtalk',
    name: 'DingTalk',
    category: 'work',
    fields: [
      { key: 'client_id', label: 'Client ID (AppKey)', required: true, type: 'text' },
      { key: 'client_secret', label: 'Client Secret', required: true, type: 'password' },
      { key: 'allowed_users', label: 'Allowed Staff IDs', required: false, type: 'list', placeholder: 'Staff IDs or *' },
    ],
  },
  {
    id: 'wecom',
    name: 'WeCom',
    category: 'work',
    fields: [
      { key: 'webhook_key', label: 'Webhook Key', required: true, type: 'password' },
      { key: 'allowed_users', label: 'Allowed User IDs', required: false, type: 'list' },
    ],
  },
  {
    id: 'qq',
    name: 'QQ Official',
    category: 'work',
    fields: [
      { key: 'app_id', label: 'App ID', required: true, type: 'text' },
      { key: 'app_secret', label: 'App Secret', required: true, type: 'password' },
      { key: 'allowed_users', label: 'Allowed User IDs', required: false, type: 'list' },
    ],
  },
  {
    id: 'nextcloud_talk',
    name: 'Nextcloud Talk',
    category: 'work',
    fields: [
      { key: 'base_url', label: 'Nextcloud URL', required: true, type: 'text', placeholder: 'https://cloud.example.com' },
      { key: 'app_token', label: 'Bot App Token', required: true, type: 'password' },
      { key: 'webhook_secret', label: 'Webhook Secret', required: false, type: 'password' },
      { key: 'allowed_users', label: 'Allowed Actor IDs', required: false, type: 'list' },
    ],
  },
];

export function getChannelsByCategory(category: ChannelCategory): ChannelDef[] {
  return CHANNELS.filter((c) => c.category === category);
}

export function getChannelById(id: string): ChannelDef | undefined {
  return CHANNELS.find((c) => c.id === id);
}
