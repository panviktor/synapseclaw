/**
 * Global in-memory chat message cache.
 * Lives outside React lifecycle — survives route changes and component unmounts.
 * Max 20 sessions cached (LRU), max 200 messages per session.
 */

interface CachedMessage {
  id: string;
  role: 'user' | 'agent';
  content: string;
  timestamp: number; // epoch ms
  kind?: string;
}

interface CachedSession {
  messages: CachedMessage[];
  lastAccess: number;
  sessionSummary?: string;
  currentGoal?: string;
}

const MAX_CACHED_SESSIONS = 20;
const MAX_MESSAGES_PER_SESSION = 200;

/** Singleton cache — lives for the page lifetime. */
const cache = new Map<string, CachedSession>();

/** Per-session draft text. */
const drafts = new Map<string, string>();

function evictLRU() {
  if (cache.size <= MAX_CACHED_SESSIONS) return;
  let oldestKey: string | null = null;
  let oldestTime = Infinity;
  for (const [key, entry] of cache) {
    if (entry.lastAccess < oldestTime) {
      oldestTime = entry.lastAccess;
      oldestKey = key;
    }
  }
  if (oldestKey) {
    cache.delete(oldestKey);
    drafts.delete(oldestKey);
  }
}

// ── Public API ──────────────────────────────────────────────────────────────

export function getCachedMessages(sessionKey: string): CachedMessage[] | null {
  const entry = cache.get(sessionKey);
  if (!entry) return null;
  entry.lastAccess = Date.now();
  return entry.messages;
}

export function setCachedMessages(
  sessionKey: string,
  messages: CachedMessage[],
  meta?: { sessionSummary?: string; currentGoal?: string },
) {
  const trimmed = messages.length > MAX_MESSAGES_PER_SESSION
    ? messages.slice(-MAX_MESSAGES_PER_SESSION)
    : messages;
  cache.set(sessionKey, {
    messages: trimmed,
    lastAccess: Date.now(),
    sessionSummary: meta?.sessionSummary,
    currentGoal: meta?.currentGoal,
  });
  evictLRU();
}

export function appendCachedMessage(sessionKey: string, msg: CachedMessage) {
  const entry = cache.get(sessionKey);
  if (!entry) {
    cache.set(sessionKey, { messages: [msg], lastAccess: Date.now() });
    evictLRU();
    return;
  }
  entry.messages.push(msg);
  if (entry.messages.length > MAX_MESSAGES_PER_SESSION) {
    entry.messages = entry.messages.slice(-MAX_MESSAGES_PER_SESSION);
  }
  entry.lastAccess = Date.now();
}

export function deleteCachedSession(sessionKey: string) {
  cache.delete(sessionKey);
  drafts.delete(sessionKey);
}

export function clearCachedSession(sessionKey: string) {
  const entry = cache.get(sessionKey);
  if (entry) {
    entry.messages = [];
    entry.lastAccess = Date.now();
  }
}

export function getCachedMeta(sessionKey: string) {
  const entry = cache.get(sessionKey);
  if (!entry) return null;
  return { sessionSummary: entry.sessionSummary, currentGoal: entry.currentGoal };
}

// ── Per-session drafts ──────────────────────────────────────────────────────

export function getSessionDraft(sessionKey: string): string {
  return drafts.get(sessionKey) ?? '';
}

export function setSessionDraft(sessionKey: string, value: string) {
  if (value) {
    drafts.set(sessionKey, value);
  } else {
    drafts.delete(sessionKey);
  }
}

export function clearSessionDraft(sessionKey: string) {
  drafts.delete(sessionKey);
}
