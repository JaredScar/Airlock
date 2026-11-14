import { Component, inject, signal } from '@angular/core';
import { DecimalPipe } from '@angular/common';
import { DESTINATIONS, StudioService } from './studio.service';
import { CATEGORY_LABEL, type Category } from './airlock.types';

const POLICY_CATEGORIES: Category[] = [
  'profanity',
  'slur',
  'spoken_pii',
  'screen_secret',
  'screen_pii',
  'tos_risk',
];

@Component({
  selector: 'app-settings',
  imports: [DecimalPipe],
  templateUrl: './settings.component.html',
})
export class SettingsComponent {
  readonly studio = inject(StudioService);
  readonly destinations = DESTINATIONS;
  readonly categories = POLICY_CATEGORIES;
  readonly label = CATEGORY_LABEL;
  readonly names: Record<(typeof DESTINATIONS)[number], string> = {
    twitch: 'Twitch',
    youtube: 'YouTube',
    kick: 'Kick',
  };
  readonly ruleDraft = signal('');
  readonly termDraft = signal('');
  readonly keyDraft = signal('');
  readonly speechKeyDraft = signal('');
  readonly streamKeyDraft = signal('');
  readonly obsPasswordDraft = signal('');
  readonly moderatorPasswordDraft = signal('');
  readonly shortcuts = [
    { id: 'veto' as const, label: 'Veto next' },
    { id: 'approve' as const, label: 'Approve next' },
    { id: 'dump' as const, label: 'Dump' },
    { id: 'safe' as const, label: 'Safe mode' },
  ];

  downloadModel() {
    void this.studio.downloadModel();
  }

  onDestination(id: 'twitch' | 'youtube' | 'kick', event: Event) {
    const input = event.target;
    if (!(input instanceof HTMLInputElement)) return;
    if (!this.studio.setDestination(id, input.checked)) input.checked = this.studio.isDestination(id);
  }

  platformName(): string {
    const id = this.studio.settings().platform;
    if (id === 'twitch' || id === 'youtube' || id === 'kick') return this.names[id];
    return 'this platform';
  }

  addRule() {
    this.studio.addRule(this.ruleDraft());
    this.ruleDraft.set('');
  }

  addTerm() {
    this.studio.addTerm(this.termDraft());
    this.termDraft.set('');
  }

  onRule(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.ruleDraft.set(input.value);
  }

  onTerm(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.termDraft.set(input.value);
  }

  onDelay(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.studio.setDelaySeconds(Number(input.value));
  }

  onStall(event: Event) {
    const input = event.target;
    if (input instanceof HTMLSelectElement) {
      const value = input.value;
      if (value === 'warn' || value === 'mute' || value === 'extend_delay') this.studio.setStall(value);
    }
  }

  onList(which: 'profanity' | 'allow', event: Event) {
    const input = event.target;
    if (!(input instanceof HTMLTextAreaElement)) return;
    const settings = this.studio.settings();
    if (which === 'profanity') this.studio.setLists(input.value, settings.extra_allow);
    else this.studio.setLists(settings.extra_profanity, input.value);
  }

  onMode(id: Category, event: Event) {
    const input = event.target;
    if (!(input instanceof HTMLSelectElement)) return;
    const value = input.value;
    if (value === 'apply' || value === 'suggest' || value === 'log_only') {
      this.studio.setCategory(id, { mode: value });
    }
  }

  onAudio(id: Category, event: Event) {
    const input = event.target;
    if (!(input instanceof HTMLSelectElement)) return;
    const value = input.value;
    if (value === 'bleep' || value === 'mute' || value === 'none') {
      this.studio.setCategory(id, { audio: value });
    }
  }

  onVideo(id: Category, event: Event) {
    const input = event.target;
    if (!(input instanceof HTMLSelectElement)) return;
    const value = input.value;
    if (value === 'blur' || value === 'pixelate' || value === 'blackout' || value === 'none') {
      this.studio.setCategory(id, { video: value });
    }
  }

  onConfidence(id: Category, event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) {
      this.studio.setCategory(id, { min_confidence: Number(input.value) });
    }
  }

  onPad(id: Category, edge: 'before' | 'after', event: Event) {
    const input = event.target;
    if (!(input instanceof HTMLInputElement)) return;
    const ms = Math.max(0, Math.round(Number(input.value) || 0));
    if (edge === 'before') this.studio.setCategory(id, { pad_before_ms: ms });
    else this.studio.setCategory(id, { pad_after_ms: ms });
  }

  onTelemetry(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.studio.setTelemetry(input.checked);
  }

  onConsent(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.studio.setConsent(input.checked);
  }

  onCloudStt(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.studio.setCloudStt(input.checked);
  }

  onSpeechKey(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.speechKeyDraft.set(input.value);
  }

  saveSpeechKey() {
    void this.studio.saveSpeechKey(this.speechKeyDraft());
    this.speechKeyDraft.set('');
  }

  onKey(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.keyDraft.set(input.value);
  }

  saveKey() {
    void this.studio.saveCloudKey(this.keyDraft());
    this.keyDraft.set('');
  }

  streamKeyNote(): string {
    if (this.studio.streamKeySaved()) return `A ${this.platformName()} stream key is saved in the OS keychain.`;
    return `Paste the ${this.platformName()} stream key from that platform's stream settings. This is not an API client secret.`;
  }

  onStreamKey(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.streamKeyDraft.set(input.value);
  }

  async saveStreamKey() {
    const draft = this.streamKeyDraft();
    await this.studio.saveStreamKey(draft);
    if (this.studio.streamKeySaved()) this.streamKeyDraft.set('');
  }

  onModerator(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.studio.setModeratorEnabled(input.checked);
  }

  onModeratorPort(event: Event) {
    const input = event.target;
    if (!(input instanceof HTMLInputElement)) return;
    const port = Number(input.value);
    if (!Number.isFinite(port)) {
      input.value = String(this.studio.settings().moderator_port);
      return;
    }
    this.studio.setModeratorPort(port);
  }

  onModeratorPassword(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.moderatorPasswordDraft.set(input.value);
  }

  saveModeratorPassword() {
    void this.studio.saveModeratorPassword(this.moderatorPasswordDraft());
    this.moderatorPasswordDraft.set('');
  }

  onStartWithObs(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.studio.setStartWithObs(input.checked);
  }

  onObsPassword(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.obsPasswordDraft.set(input.value);
  }

  saveObsPassword() {
    void this.studio.saveObsPassword(this.obsPasswordDraft());
    this.obsPasswordDraft.set('');
  }

  onSourceScan(name: string, event: Event) {
    const input = event.target;
    if (!(input instanceof HTMLSelectElement)) return;
    const value = input.value;
    if (value === 'always' || value === 'never' || value === 'default') {
      this.studio.setSourceScan(name, value);
    }
  }

  shortcutValue(id: 'veto' | 'approve' | 'dump' | 'safe'): string {
    const settings = this.studio.settings();
    if (id === 'approve') return settings.shortcut_approve;
    if (id === 'dump') return settings.shortcut_dump;
    if (id === 'safe') return settings.shortcut_safe;
    return settings.shortcut_veto;
  }

  onShortcut(id: 'veto' | 'approve' | 'dump' | 'safe', event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.studio.setShortcut(id, input.value);
  }
}
