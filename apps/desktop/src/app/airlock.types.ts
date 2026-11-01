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
  let cursor = 0;
  return words.map((word) => {
    cursor = Math.max(cursor, word.span.start);
    const hasAired = cursor <= playhead;
    const overlaps = (redaction: Redaction) => {
      if (redaction.target.kind !== 'audio') return false;
      const mid = word.span.start + (word.span.end - word.span.start) / 2;
      const coversMid = mid >= redaction.span.start && mid < redaction.span.end;
      const covered =
        Math.min(redaction.span.end, word.span.end) - Math.max(redaction.span.start, word.span.start);
      return coversMid && covered >= (word.span.end - word.span.start) / 2;
    };
    const hit = upcoming.find(overlaps) ?? aired.find(overlaps);
    const state = hit ? (isActive(hit) ? 'redacted' : 'flagged') : hasAired ? 'aired' : 'held';
    return hit && hasAired ? `${state} aired` : state;
  });
}
