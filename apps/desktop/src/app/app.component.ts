import { Component, HostListener, computed, inject, signal } from '@angular/core';
import { DecimalPipe } from '@angular/common';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { AirlockService, IncidentFormat, inDesktopShell } from './airlock.service';
import { CATEGORY_LABEL, Redaction, isActive, transcriptStates } from './airlock.types';
import { ModeratorComponent } from './moderator.component';
import { SettingsComponent } from './settings.component';
import { WizardComponent } from './wizard.component';
import { StudioService } from './studio.service';

@Component({
  selector: 'app-root',
  imports: [DecimalPipe, SettingsComponent, WizardComponent, ModeratorComponent],
  templateUrl: './app.component.html',
  host: {
    '[class.compact]': 'compact()',
  },
})
export class AppComponent {
  readonly al = inject(AirlockService);
  readonly studio = inject(StudioService);
  readonly label = CATEGORY_LABEL;
  readonly isActive = isActive;
  readonly desktop = inDesktopShell();
  readonly page = signal<'live' | 'settings' | 'setup' | 'moderator'>('live');
  readonly compact = signal(false);
  readonly light = signal(false);
  readonly safe = signal(false);
  readonly shareCorrections = signal(false);

  readonly delayFill = computed(() => {
    const hold = this.studio.settings().policy.delay_ms;
    if (hold <= 0) return 0;
    return Math.min(100, (this.al.delayMs() / hold) * 100);
  });
  readonly blackoutVideo = signal(false);
  readonly demoMode = signal(false);

  platformName(): string {
    return this.studio.destinationLabel();
  }

  dump() {
    this.al.dump(this.blackoutVideo());
  }

  setBlackout(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.blackoutVideo.set(input.checked);
  }

  setShare(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.shareCorrections.set(input.checked);
  }

  constructor() {
    const theme = localStorage.getItem('airlock.theme');
    if (theme === 'light') {
      document.documentElement.dataset['theme'] = 'light';
      this.light.set(true);
    }
    if (inDesktopShell()) {
      void this.al.ensureListening();
      void listen<string>('airlock://shortcut', (event) => {
        if (event.payload === 'veto') this.actNext('veto');
        else if (event.payload === 'approve') this.actNext('approve');
        else if (event.payload === 'dump') this.dump();
        else if (event.payload === 'safe') this.toggleSafe();
      });
    }
  }

  ingestLabel(): string {
    const connected = this.al.ingest();
    if (connected === null) return '—';
    return connected ? 'Connected' : 'Disconnected';
  }

  ingestTitle(): string {
    return this.ingestLabel() === '—' ? 'Shown when Go live is running' : 'Local publisher';
  }

  setDemo(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.demoMode.set(input.checked);
  }

  toggleLive() {
    if (this.al.running()) void this.al.stopLive();
    else void this.al.startLive();
  }

  toggleTheme() {
    const on = !this.light();
    this.light.set(on);
    if (on) document.documentElement.dataset['theme'] = 'light';
    else delete document.documentElement.dataset['theme'];
    localStorage.setItem('airlock.theme', on ? 'light' : 'dark');
  }

  toggleCompact() {
    const on = !this.compact();
    this.compact.set(on);
    if (inDesktopShell()) void invoke('set_compact', { on });
  }

  toggleSafe() {
    const on = !this.safe();
    this.safe.set(on);
    if (inDesktopShell()) {
      void invoke('set_safe_mode', { on });
    } else if (on) {
      this.al.dump(false);
    }
  }

  markWrong(row: Redaction) {
    const line = JSON.stringify({
      clip: 'correction',
      category: row.category,
      start_us: row.span.start,
      end_us: row.span.end,
      note: 'marked wrong by the streamer',
    });
    const blob = new Blob([line + '\n'], { type: 'application/jsonl' });
    const link = document.createElement('a');
    link.href = URL.createObjectURL(blob);
    link.download = 'airlock-correction.jsonl';
    link.click();
    URL.revokeObjectURL(link.href);
  }

  @HostListener('window:keydown', ['$event'])
  onKey(event: KeyboardEvent) {
    const target = event.target;
    if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) {
      return;
    }
    const key = event.key.length === 1 ? event.key.toUpperCase() : event.key;
    const settings = this.studio.settings();
    const match = (spec: string) => spec.toLowerCase() === key.toLowerCase();
    if (match(settings.shortcut_veto)) this.actNext('veto');
    else if (match(settings.shortcut_approve)) this.actNext('approve');
    else if (match(settings.shortcut_dump)) this.dump();
    else if (match(settings.shortcut_safe)) this.toggleSafe();
    else return;
    event.preventDefault();
  }

  private actNext(which: 'approve' | 'veto') {
    const next = this.al.upcoming()[0];
    if (!next) return;
    if (which === 'approve') void this.al.approve(next.id);
    else void this.al.veto(next.id);
  }

  secondsUntilAir(r: Redaction): number {
    return Math.max(0, (r.span.start - this.al.playhead()) / 1e6);
  }

  /** Where a redaction sits on the tape: 0 is about to air, 100 is the live edge. */
  tickAt(r: Redaction): number {
    const held = this.al.live() - this.al.playhead();
    if (held <= 0) return 0;
    return Math.min(100, Math.max(0, ((r.span.start - this.al.playhead()) / held) * 100));
  }

  isAiring(r: Redaction): boolean {
    return r.span.start <= this.al.playhead() && r.span.end > this.al.playhead();
  }

  readonly transcriptState = computed(() =>
    transcriptStates(this.al.words(), this.al.playhead(), this.al.upcoming(), this.al.aired()),
  );

  /** Word state for the transcript: flagged words are highlighted by their redaction. */
  wordState(index: number): string {
    return this.transcriptState()[index] ?? 'held';
  }

  formatTs(us: number): string {
    const s = Math.max(0, Math.floor(us / 1e6));
    return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
  }

  async download(kind: IncidentFormat) {
    const text = await this.al.exportIncidents(kind);
    const file = {
      csv: ['airlock-incidents.csv', 'text/csv'],
      edl: ['airlock-incidents.edl', 'text/plain'],
      chapters: ['airlock-chapters.vtt', 'text/vtt'],
      youtube: ['airlock-youtube.txt', 'text/plain'],
    }[kind];
    const url = URL.createObjectURL(new Blob([text], { type: file[1] }));
    const a = Object.assign(document.createElement('a'), { href: url, download: file[0] });
    a.click();
    URL.revokeObjectURL(url);
  }
}
