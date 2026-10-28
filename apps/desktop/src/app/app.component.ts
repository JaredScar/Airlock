import { Component, computed, inject } from '@angular/core';
import { DecimalPipe } from '@angular/common';
import { AirlockService } from './airlock.service';
import { CATEGORY_LABEL, Redaction, Word, isActive } from './airlock.types';

@Component({
  selector: 'app-root',
  imports: [DecimalPipe],
  templateUrl: './app.component.html',
})
export class AppComponent {
  readonly al = inject(AirlockService);
  readonly label = CATEGORY_LABEL;
  readonly isActive = isActive;

  /** Delay meter fill: how full the airlock is relative to the configured delay. */
  readonly delayFill = computed(() => Math.min(100, (this.al.delayMs() / 8000) * 100));

  constructor() {
    this.al.ensureListening();
  }

  secondsUntilAir(r: Redaction): number {
    return Math.max(0, (r.span.start - this.al.playhead()) / 1e6);
  }

  isAiring(r: Redaction): boolean {
    return r.span.start <= this.al.playhead() && r.span.end > this.al.playhead();
  }

  /** Word state for the transcript: flagged words are highlighted by their redaction. */
  wordState(w: Word): string {
    // Count a word as covered only if most of it is, so time padding doesn't mark neighbors.
    const overlaps = (r: Redaction) => {
      if (r.target.kind !== 'audio') return false;
      const covered = Math.min(r.span.end, w.span.end) - Math.max(r.span.start, w.span.start);
      return covered >= (w.span.end - w.span.start) / 2;
    };
    const aired = w.span.end <= this.al.playhead();
    const hit = this.al.upcoming().find(overlaps) ?? this.al.aired().find(overlaps);
    const state = hit ? (isActive(hit) ? 'redacted' : 'flagged') : aired ? 'aired' : 'held';
    return hit && aired ? `${state} aired` : state;
  }

  formatTs(us: number): string {
    const s = Math.max(0, Math.floor(us / 1e6));
    return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
  }

  async exportCsv() {
    const csv = await this.al.exportCsv();
    const url = URL.createObjectURL(new Blob([csv], { type: 'text/csv' }));
    const a = Object.assign(document.createElement('a'), { href: url, download: 'airlock-incidents.csv' });
    a.click();
    URL.revokeObjectURL(url);
  }
}
