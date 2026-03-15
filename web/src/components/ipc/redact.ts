/**
 * Redact payload for admin UI display.
 *
 * Default (table/preview) mode: no raw content visible at all.
 * Shows only message kind hint (if detectable) and byte length.
 * Raw content is only revealed via explicit "Show raw" toggle in MessageDetail.
 */
export function redactPayload(payload: string, kind?: string): string {
  const len = payload.length;
  if (len === 0) return '[empty]';
  const label = kind ? `${kind}: ` : '';
  return `[${label}${len} chars]`;
}
