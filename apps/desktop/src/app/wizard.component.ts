import { Component, inject, signal } from '@angular/core';
import { invoke } from '@tauri-apps/api/core';
import { DESTINATIONS, StudioService } from './studio.service';
import { inDesktopShell } from './airlock.service';

@Component({
  selector: 'app-wizard',
  templateUrl: './wizard.component.html',
})
export class WizardComponent {
  readonly studio = inject(StudioService);
  readonly destinations = DESTINATIONS;
  readonly names: Record<(typeof DESTINATIONS)[number], string> = {
    twitch: 'Twitch',
    youtube: 'YouTube',
    kick: 'Kick',
  };
  readonly desktop = inDesktopShell();
  readonly gpu = signal<string>('Checking this PC.');
  readonly model = signal('');
  readonly streamKeyDraft = signal('');
  readonly obsPasswordDraft = signal('');
  readonly obsNote = signal('OBS WebSocket has to be enabled. The password stays in the OS keychain.');

  constructor() {
    void this.loadMachine();
  }

  async loadMachine() {
    if (!this.desktop) {
      this.gpu.set('Open the Airlock app to detect the GPU.');
      return;
    }
    try {
      const setup = await invoke<{ gpu_mb: number | null; model: string }>('machine_setup');
      this.gpu.set(setup.gpu_mb ? `${Math.round(setup.gpu_mb / 1024)} GB GPU` : 'No NVIDIA GPU detected.');
      this.model.set(setup.model);
    } catch (e) {
      this.gpu.set(String(e));
    }
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

  onStreamKey(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.streamKeyDraft.set(input.value);
  }

  async saveStreamKey() {
    await this.studio.saveStreamKey(this.streamKeyDraft());
    if (this.studio.streamKeySaved()) this.streamKeyDraft.set('');
  }

  onObsPassword(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.obsPasswordDraft.set(input.value);
  }

  async saveObsPassword() {
    await this.studio.saveObsPassword(this.obsPasswordDraft());
    this.obsPasswordDraft.set('');
    this.obsNote.set(this.studio.obsNote());
  }

  async pointObs() {
    if (!this.desktop) {
      this.obsNote.set('Open the Airlock app to configure OBS.');
      return;
    }
    try {
      await invoke('configure_obs_ingest');
      this.obsNote.set('OBS is pointed at rtmp://127.0.0.1:1935/live with the local key.');
    } catch (e) {
      this.obsNote.set(String(e));
    }
  }
}
