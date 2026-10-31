import { Injectable, NgZone, computed, inject, signal } from '@angular/core';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import fixture from './fixtures/demo-events.json';
import { Micros, Redaction, Status, UiEvent, Word } from './airlock.types';

const EVENT_CHANNEL = 'airlock://event';
const TRANSCRIPT_KEEP = 120;

interface TimedEvent {
  at_ms: number;
  event: UiEvent;
}

const PREVIEW: TimedEvent[] = fixture as TimedEvent[];

export type IncidentFormat = 'csv' | 'edl' | 'youtube' | 'chapters';

/** True when the page is hosted by the Tauri shell rather than `ng serve`. */
export function inDesktopShell(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** Single source of truth for live session state in the UI. */
@Injectable({ providedIn: 'root' })
export class AirlockService {
  private zone = inject(NgZone);

  readonly live = signal<Micros>(0);
  readonly playhead = signal<Micros>(0);
  readonly words = signal<Word[]>([]);
  readonly pending = signal<Map<number, Redaction>>(new Map());
  readonly aired = signal<Redaction[]>([]);
  readonly running = signal(false);
  readonly ingest = signal<boolean | null>(null);
  readonly error = signal<string | null>(null);
  readonly stall = signal<string | null>(null);

  /** Redactions still inside the delay window, soonest first. */
  readonly upcoming = computed(() =>
    [...this.pending().values()].filter((r) => r.span.end > this.playhead()).sort((a, b) => a.span.start - b.span.start),
  );

  readonly delayMs = computed(() => Math.max(0, (this.live() - this.playhead()) / 1000));

  private listening = false;
  private previewTimer: ReturnType<typeof setTimeout> | null = null;

  async ensureListening(): Promise<void> {
    if (this.listening || !inDesktopShell()) return;
    this.listening = true;
    await listen<UiEvent>(EVENT_CHANNEL, (e) => this.zone.run(() => this.apply(e.payload)));
  }

  async startLive(): Promise<void> {
    if (!inDesktopShell()) {
      this.error.set('Open the Airlock app to go live.');
      return;
    }
    this.stopFixture();
    await this.ensureListening();
    this.reset();
    this.running.set(true);
    this.ingest.set(false);
    try {
      await invoke('start_live');
    } catch (e) {
      this.running.set(false);
      this.ingest.set(null);
      this.error.set(String(e));
    }
  }

  async stopLive(): Promise<void> {
    if (!inDesktopShell()) {
      this.running.set(false);
      this.ingest.set(false);
      return;
    }
    try {
      await invoke('stop_live');
    } catch (e) {
      this.error.set(String(e));
    }
    this.running.set(false);
    this.ingest.set(false);
  }

  async startDemo(): Promise<void> {
    if (!inDesktopShell()) {
      this.playFixture();
      return;
    }
    this.stopFixture();
    await this.ensureListening();
    this.reset();
    this.running.set(true);
    await invoke('start_demo');
  }

  /** Replays the recorded demo through the same UI state the desktop app uses. */
  playFixture(): void {
    this.stopFixture();
    this.reset();
    this.running.set(true);
    const started = performance.now();
    let index = 0;
    const tick = () => {
      const elapsed = performance.now() - started;
      while (index < PREVIEW.length && PREVIEW[index].at_ms <= elapsed) {
        this.apply(PREVIEW[index].event);
        index++;
      }
      if (index < PREVIEW.length && this.running()) {
        this.previewTimer = setTimeout(tick, 50);
      }
    };
    tick();
  }

  approve(id: number) {
    if (!inDesktopShell()) {
      this.patchStatus(id, 'approved');
      return;
    }
    return this.call('approve', { id });
  }

  veto(id: number) {
    if (!inDesktopShell()) {
      this.patchStatus(id, 'vetoed');
      return;
    }
    return this.call('veto', { id });
  }

  dump(blackoutVideo: boolean) {
    if (!inDesktopShell()) {
      this.localDump(blackoutVideo);
      return;
    }
    return this.call('dump', { blackout_video: blackoutVideo });
  }

  exportIncidents(format: IncidentFormat): Promise<string> {
    if (!inDesktopShell()) return Promise.resolve(previewExport(this.aired(), format));
    return invoke<string>('export_incidents', { format });
  }

  private async call(cmd: string, args: Record<string, unknown>) {
    try {
      await invoke(cmd, args);
    } catch (e) {
      this.error.set(String(e));
    }
  }

  private stopFixture() {
    if (this.previewTimer != null) {
      clearTimeout(this.previewTimer);
      this.previewTimer = null;
    }
  }

  private patchStatus(id: number, status: Status) {
    this.pending.update((m) => {
      const current = m.get(id);
      if (!current) return m;
      const next = new Map(m);
      next.set(id, { ...current, status });
      return next;
    });
  }

  private localDump(blackoutVideo: boolean) {
    const live = this.live();
    const start = Math.max(0, Math.min(this.playhead(), live));
    const end = live + 1;
    this.insertLocal({
      action: 'mute',
      target: { kind: 'audio' },
      span: { start, end },
      reasons: ['Dump'],
    });
    if (blackoutVideo) {
      this.insertLocal({
        action: 'blackout',
        target: { kind: 'video', region: { x: 0, y: 0, w: 1, h: 1 } },
        span: { start, end },
        reasons: ['Dump'],
      });
    }
  }

  private insertLocal(partial: Pick<Redaction, 'action' | 'target' | 'span' | 'reasons'>) {
    const used = [...this.pending().keys(), ...this.aired().map((r) => r.id)];
    const id = used.reduce((m, n) => Math.max(m, n), 0) + 1;
    const redaction: Redaction = {
      id,
      category: 'manual',
      mode: 'apply',
      status: 'pending',
      confidence: 1,
      sources: ['manual'],
      evidence: [],
      missed_live: false,
      ...partial,
    };
    this.pending.update((m) => new Map(m).set(id, redaction));
  }

  private reset() {
    this.live.set(0);
    this.playhead.set(0);
    this.words.set([]);
    this.pending.set(new Map());
    this.aired.set([]);
    this.error.set(null);
    this.stall.set(null);
    this.ingest.set(null);
  }

  private apply(ev: UiEvent) {
    switch (ev.type) {
      case 'transcript':
        this.words.update((w) => [...w, ...ev.words].slice(-TRANSCRIPT_KEEP));
        break;
      case 'redaction_upserted':
        this.pending.update((m) => new Map(m).set(ev.redaction.id, ev.redaction));
        break;
      case 'redaction_aired':
        this.pending.update((m) => {
          const next = new Map(m);
          next.delete(ev.redaction.id);
          return next;
        });
        this.aired.update((a) => [ev.redaction, ...a]);
        break;
      case 'clock':
        this.live.set(ev.live);
        this.playhead.set(ev.playhead);
        break;
      case 'analysis_stall':
        this.stall.set(ev.active ? ev.message : null);
        break;
      case 'ingest':
        this.ingest.set(ev.connected);
        break;
      default: {
        const unexpected: never = ev;
        throw new Error(`unhandled event ${JSON.stringify(unexpected)}`);
      }
    }
  }
}

function previewExport(items: Redaction[], format: IncidentFormat): string {
  const ordered = [...items].sort((a, b) => a.span.start - b.span.start);
  switch (format) {
    case 'csv':
      return previewCsv(items);
    case 'edl':
      return previewEdl(ordered);
    case 'youtube':
      return previewYoutube(ordered);
    case 'chapters':
      return previewChapters(ordered);
  }
}

function incidentNote(r: Redaction): string {
  return r.missed_live ? `${r.category} — ${r.action} — missed live` : `${r.category} — ${r.action}`;
}

function previewCsv(items: Redaction[]): string {
  const rows = ['start,end,category,action,status,confidence,reasons,evidence,missed_live'];
  for (const r of items) {
    rows.push(
      [
        r.span.start,
        r.span.end,
        r.category,
        r.action,
        r.status,
        r.confidence.toFixed(2),
        csvField(r.reasons.join('; ')),
        csvField(r.evidence.join('; ')),
        r.missed_live ? 'true' : 'false',
      ].join(','),
    );
  }
  return rows.join('\n') + '\n';
}

function previewEdl(items: Redaction[]): string {
  const lines = ['TITLE: Airlock incidents', 'FCM: NON-DROP FRAME', ''];
  items.forEach((r, i) => {
    const track = r.target.kind === 'audio' ? 'A' : 'V';
    const inn = edlTc(r.span.start);
    const out = edlTc(Math.max(r.span.end, r.span.start + 1));
    const n = String(i + 1).padStart(3, '0');
    lines.push(`${n}  AX       ${track.padEnd(6, ' ')}C        ${inn} ${out} ${inn} ${out}`);
    lines.push(`* FROM CLIP NAME: ${incidentNote(r)}`);
  });
  return lines.join('\n') + '\n';
}

function previewYoutube(items: Redaction[]): string {
  const lines: string[] = [];
  if (items.length === 0 || Math.floor(items[0].span.start / 1_000_000) !== 0) {
    lines.push('0:00 Start');
  }
  for (const r of items) lines.push(`${youtubeClock(r.span.start)} ${incidentNote(r)}`);
  return lines.join('\n') + '\n';
}

function previewChapters(items: Redaction[]): string {
  let out = 'WEBVTT\n\n';
  for (const r of items) {
    const end = Math.max(r.span.end, r.span.start + 1000);
    out += `${vttClock(r.span.start)} --> ${vttClock(end)}\n${incidentNote(r)}\n\n`;
  }
  return out;
}

function edlTc(us: number): string {
  const framesTotal = Math.floor((Math.max(0, us) * 30) / 1_000_000);
  const frames = framesTotal % 30;
  const totalSecs = Math.floor(framesTotal / 30);
  const s = totalSecs % 60;
  const m = Math.floor(totalSecs / 60) % 60;
  const h = Math.floor(totalSecs / 3600);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(h)}:${p(m)}:${p(s)}:${p(frames)}`;
}

function youtubeClock(us: number): string {
  const secs = Math.floor(Math.max(0, us) / 1_000_000);
  const h = Math.floor(secs / 3600);
  const m = Math.floor(secs / 60) % 60;
  const s = secs % 60;
  const p = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${p(m)}:${p(s)}` : `${m}:${p(s)}`;
}

function vttClock(us: number): string {
  const total = Math.max(0, Math.floor(us / 1000));
  const h = Math.floor(total / 3_600_000);
  const m = Math.floor(total / 60_000) % 60;
  const s = Math.floor(total / 1000) % 60;
  const ms = total % 1000;
  const p = (n: number, w = 2) => String(n).padStart(w, '0');
  return `${p(h)}:${p(m)}:${p(s)}.${p(ms, 3)}`;
}

function csvField(s: string): string {
  return /[",\n]/.test(s) ? `"${s.replaceAll('"', '""')}"` : s;
}
