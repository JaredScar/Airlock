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
  if (r.action === 'log_only') return false;
  if (r.mode === 'apply') return r.status !== 'vetoed';
  if (r.mode === 'suggest') return r.status === 'approved';
  return false;
}
