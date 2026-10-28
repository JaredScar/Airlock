// Mirrors the serde output of airlock-core / airlock-pipeline. Times are microseconds
// of media time since stream start. Keep in sync with the Rust types (PLAN.md 5.x
// suggests generating these with ts-rs or specta).

export type Micros = number;

export type Category = 'profanity' | 'slur' | 'spoken_pii' | 'screen_secret' | 'screen_pii' | 'tos_risk';
export type Mode = 'apply' | 'suggest' | 'log_only';
export type Action = 'bleep' | 'mute' | 'blur' | 'pixelate' | 'blackout' | 'log_only';
export type Status = 'pending' | 'approved' | 'vetoed';

export interface Span {
  start: Micros;
  end: Micros;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type Target = { kind: 'audio' } | { kind: 'video'; region: Rect };

export interface Word {
  text: string;
  span: Span;
  confidence: number;
}

export interface Redaction {
  id: number;
  category: Category;
  mode: Mode;
  action: Action;
  target: Target;
  span: Span;
  status: Status;
  confidence: number;
  sources: string[];
  reasons: string[];
  evidence: string[];
}

export type UiEvent =
  | { type: 'transcript'; words: Word[] }
  | { type: 'redaction_upserted'; redaction: Redaction }
  | { type: 'redaction_aired'; redaction: Redaction }
  | { type: 'clock'; live: Micros; playhead: Micros };

export const CATEGORY_LABEL: Record<Category, string> = {
  profanity: 'Profanity',
  slur: 'Slur',
  spoken_pii: 'Spoken PII',
  screen_secret: 'Secret on screen',
  screen_pii: 'PII on screen',
  tos_risk: 'ToS risk',
};

/** Mirrors `Redaction::is_active` in airlock-core. */
export function isActive(r: Redaction): boolean {
  if (r.action === 'log_only') return false;
  if (r.mode === 'apply') return r.status !== 'vetoed';
  if (r.mode === 'suggest') return r.status === 'approved';
  return false;
}
