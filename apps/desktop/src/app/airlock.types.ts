// Generated from the Rust types (ts-rs). `npm run build` regenerates `./generated`
// before `ng build`, so a renamed or removed Rust field fails this build until the
// UI is updated. Helpers below are UI-only.

import type { Category } from './generated/Category';
import type { Redaction } from './generated/Redaction';
import type { Ts } from './generated/Ts';

export type { Action } from './generated/Action';
export type { Category } from './generated/Category';
export type { DetectorKind } from './generated/DetectorKind';
export type { Mode } from './generated/Mode';
export type { Rect } from './generated/Rect';
export type { Redaction } from './generated/Redaction';
export type { Span } from './generated/Span';
export type { Status } from './generated/Status';
export type { Target } from './generated/Target';
export type { Ts } from './generated/Ts';
export type { UiEvent } from './generated/UiEvent';
export type { Word } from './generated/Word';
export type { Platform } from './generated/Platform';
export type { Policy } from './generated/Policy';
export type { CategoryPolicy } from './generated/CategoryPolicy';
export type { StudioSettings } from './generated/StudioSettings';
export type { AudioAction } from './generated/AudioAction';
export type { VideoAction } from './generated/VideoAction';
export type { StallFallback } from './generated/StallFallback';
/** Media-time microseconds. Same JSON number as `Ts`. */
export type Micros = Ts;

export const CATEGORY_LABEL: Record<Category, string> = {
  profanity: 'Profanity',
  slur: 'Slur',
  spoken_pii: 'Spoken PII',
  screen_secret: 'Secret on screen',
  screen_pii: 'PII on screen',
  tos_risk: 'ToS risk',
  manual: 'Dump',
};

/** Mirrors `Redaction::is_active` in airlock-core. */
export function isActive(r: Redaction): boolean {
  if (r.missed_live || r.action === 'log_only') return false;
  if (r.mode === 'apply') return r.status !== 'vetoed';
  if (r.mode === 'suggest') return r.status === 'approved';
  return false;
}

/**
 * Transcript colors in reading order. A word changes when it starts to air,
 * and never before the word in front of it, even if its timestamp is earlier.
 */
export function transcriptStates(
  words: { text: string; span: { start: number; end: number } }[],
  playhead: number,
  upcoming: Redaction[],
  aired: Redaction[],
): string[] {
  const owned = new Map<number, Redaction>();
  for (const redaction of [...upcoming, ...aired]) {
    if (redaction.target.kind !== 'audio') continue;
    const index = listHitOwner(redaction, words);
    if (index >= 0) owned.set(index, redaction);
  }
  let cursor = 0;
  return words.map((word, index) => {
    cursor = Math.max(cursor, word.span.start);
    const hasAired = cursor <= playhead;
    const overlaps = (redaction: Redaction) => {
      if (redaction.target.kind !== 'audio') return false;
      if (redaction.category === 'profanity' || redaction.category === 'slur') return false;
      const mid = word.span.start + (word.span.end - word.span.start) / 2;
      const coversMid = mid >= redaction.span.start && mid < redaction.span.end;
      const wordLen = Math.max(word.span.end - word.span.start, 1);
      const covered =
        Math.min(redaction.span.end, word.span.end) - Math.max(redaction.span.start, word.span.start);
      return coversMid && covered >= wordLen / 2;
    };
    const hit = owned.get(index) ?? upcoming.find(overlaps) ?? aired.find(overlaps);
    const state = hit ? (isActive(hit) ? 'redacted' : 'flagged') : hasAired ? 'aired' : 'held';
    return hit && hasAired ? `${state} aired` : state;
  });
}

/** The bleep belongs to the word whose text matches it and whose span holds its middle. */
function listHitOwner(
  redaction: Redaction,
  words: { text: string; span: { start: number; end: number } }[],
): number {
  if (redaction.category !== 'profanity' && redaction.category !== 'slur') {
    return -1;
  }
  const mid = (redaction.span.start + redaction.span.end) / 2;
  let best = -1;
  let bestScore = 0;
  let bestLen = Number.POSITIVE_INFINITY;
  words.forEach((word, index) => {
    if (mid < word.span.start || mid >= word.span.end) return;
    const score = evidenceScore(word.text, redaction.evidence);
    const len = word.span.end - word.span.start;
    if (score > bestScore || (score === bestScore && score > 0 && len < bestLen)) {
      best = index;
      bestScore = score;
      bestLen = len;
    }
  });
  return best;
}

function evidenceScore(text: string, evidence: string[]): number {
  const token = text.toLowerCase().replace(/[^a-z]/g, '');
  let best = 0;
  for (const item of evidence) {
    const prefix = item.toLowerCase().replace(/[^a-z]/g, '');
    if (prefix.length > 0 && token.startsWith(prefix)) {
      best = Math.max(best, prefix.length);
    }
  }
  return best;
}
