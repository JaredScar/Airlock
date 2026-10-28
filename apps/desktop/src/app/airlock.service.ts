import { Injectable, NgZone, computed, inject, signal } from '@angular/core';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Micros, Redaction, UiEvent, Word } from './airlock.types';

const EVENT_CHANNEL = 'airlock://event';
const TRANSCRIPT_KEEP = 120;

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
  readonly error = signal<string | null>(null);

  /** Redactions still inside the delay window, soonest first. */
  readonly upcoming = computed(() =>
    [...this.pending().values()].filter((r) => r.span.end > this.playhead()).sort((a, b) => a.span.start - b.span.start),
  );

  readonly delayMs = computed(() => Math.max(0, (this.live() - this.playhead()) / 1000));

  private listening = false;

  async ensureListening(): Promise<void> {
    if (this.listening || !isTauri()) return;
    this.listening = true;
    await listen<UiEvent>(EVENT_CHANNEL, (e) => this.zone.run(() => this.apply(e.payload)));
  }

  async startDemo(): Promise<void> {
    if (!isTauri()) {
      this.error.set('Run inside the desktop app (npm run tauri dev) to use the engine.');
      return;
    }
    await this.ensureListening();
    this.reset();
    this.running.set(true);
    await invoke('start_demo');
  }

  approve(id: number) {
    return this.call('approve', { id });
  }

  veto(id: number) {
    return this.call('veto', { id });
  }

  exportCsv(): Promise<string> {
    return invoke<string>('export_incidents_csv');
  }

  private async call(cmd: string, args: Record<string, unknown>) {
    try {
      await invoke(cmd, args);
    } catch (e) {
      this.error.set(String(e));
    }
  }

  private reset() {
    this.live.set(0);
    this.playhead.set(0);
    this.words.set([]);
    this.pending.set(new Map());
    this.aired.set([]);
    this.error.set(null);
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
    }
  }
}

function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
