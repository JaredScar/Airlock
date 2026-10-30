import { Injectable, signal } from '@angular/core';
import { invoke } from '@tauri-apps/api/core';
import { inDesktopShell } from './airlock.service';
import type { Category, CategoryPolicy, StudioSettings } from './airlock.types';

const STORAGE_KEY = 'airlock.studio.v1';

/** Mirrors `Policy::default()` and `StudioSettings::default()`. */
export const DEFAULT_STUDIO: StudioSettings = {
  platform: 'twitch',
  rules_twitch: [],
  rules_youtube: [],
  rules_kick: [],
  private_terms: ['Springfield High'],
  extra_profanity: '',
  extra_allow: '',
  cloud_judge_consent: false,
  shortcut_veto: 'F8',
  shortcut_approve: 'F9',
  shortcut_dump: 'F10',
  shortcut_safe: 'F11',
  policy: {
    delay_ms: 8000,
    merge_gap_ms: 250,
    stall_after_ms: 3000,
    stall_fallback: 'warn',
    categories: {
      profanity: { mode: 'apply', audio: 'bleep', video: 'none', min_confidence: 0.6, pad_before_ms: 80, pad_after_ms: 80 },
      slur: { mode: 'apply', audio: 'bleep', video: 'none', min_confidence: 0.5, pad_before_ms: 120, pad_after_ms: 120 },
      spoken_pii: { mode: 'apply', audio: 'mute', video: 'none', min_confidence: 0.6, pad_before_ms: 150, pad_after_ms: 150 },
      screen_secret: { mode: 'apply', audio: 'none', video: 'blur', min_confidence: 0.5, pad_before_ms: 500, pad_after_ms: 750 },
      screen_pii: { mode: 'suggest', audio: 'none', video: 'blur', min_confidence: 0.6, pad_before_ms: 500, pad_after_ms: 750 },
      tos_risk: { mode: 'suggest', audio: 'mute', video: 'none', min_confidence: 0.75, pad_before_ms: 200, pad_after_ms: 200 },
    },
  },
};

export const DESTINATIONS = ['twitch', 'youtube', 'kick'] as const;

@Injectable({ providedIn: 'root' })
export class StudioService {
  readonly settings = signal<StudioSettings>(loadStudio());
  readonly pack = signal('');
  readonly error = signal<string | null>(null);

  constructor() {
    if (inDesktopShell()) void this.pullFromDesktop();
    void this.refreshPack();
  }

  rulesFor(platform = this.settings().platform): string[] {
    if (platform === 'twitch') return this.settings().rules_twitch;
    if (platform === 'youtube') return this.settings().rules_youtube;
    if (platform === 'kick') return this.settings().rules_kick;
    return [];
  }

  category(id: Category): CategoryPolicy | undefined {
    return this.settings().policy.categories[id];
  }

  choosePlatform(platform: StudioSettings['platform']) {
    this.commit({ ...this.settings(), platform });
  }

  addRule(text: string) {
    const rule = text.trim();
    const platform = this.settings().platform;
    if (!rule || platform === 'generic') return;
    const key = ruleKey(platform);
    this.commit({ ...this.settings(), [key]: [...this.settings()[key], rule] });
  }

  removeRule(index: number) {
    const platform = this.settings().platform;
    if (platform === 'generic') return;
    const key = ruleKey(platform);
    this.commit({ ...this.settings(), [key]: this.settings()[key].filter((_, i) => i !== index) });
  }

  addTerm(text: string) {
    const term = text.trim();
    if (!term || this.settings().private_terms.some((t) => t.toLowerCase() === term.toLowerCase())) return;
    this.commit({ ...this.settings(), private_terms: [...this.settings().private_terms, term] });
  }

  removeTerm(index: number) {
    this.commit({
      ...this.settings(),
      private_terms: this.settings().private_terms.filter((_, i) => i !== index),
    });
  }

  setDelaySeconds(seconds: number) {
    const delay_ms = Math.round(seconds * 1000);
    this.commit({ ...this.settings(), policy: { ...this.settings().policy, delay_ms } });
  }

  setStall(stall_fallback: StudioSettings['policy']['stall_fallback']) {
    this.commit({ ...this.settings(), policy: { ...this.settings().policy, stall_fallback } });
  }

  setCategory(id: Category, patch: Partial<CategoryPolicy>) {
    const current = this.category(id);
    if (!current) return;
    this.commit({
      ...this.settings(),
      policy: {
        ...this.settings().policy,
        categories: { ...this.settings().policy.categories, [id]: { ...current, ...patch } },
      },
    });
  }

  setLists(extra_profanity: string, extra_allow: string) {
    this.commit({ ...this.settings(), extra_profanity, extra_allow });
  }

  setConsent(cloud_judge_consent: boolean) {
    this.commit({ ...this.settings(), cloud_judge_consent });
  }

  setShortcut(which: 'veto' | 'approve' | 'dump' | 'safe', value: string) {
    const shortcut = value.trim() || DEFAULT_STUDIO[`shortcut_${which}`];
    this.commit({ ...this.settings(), [`shortcut_${which}`]: shortcut });
  }

  async saveCloudKey(key: string) {
    if (!inDesktopShell()) {
      this.error.set('The preview cannot store a cloud key. Open the desktop app.');
      return;
    }
    try {
      await invoke('set_cloud_key', { key });
      this.error.set(null);
    } catch (e) {
      this.error.set(String(e));
    }
  }

  reset() {
    this.commit(structuredClone(DEFAULT_STUDIO));
  }

  private commit(next: StudioSettings) {
    this.settings.set(next);
    const stored = inDesktopShell() ? { ...next, private_terms: [] as string[] } : next;
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(stored));
    } catch {
      this.error.set('Could not store settings in this browser.');
    }
    void this.pushToDesktop(next);
    void this.refreshPack();
  }

  private async pullFromDesktop() {
    try {
      const remote = await invoke<StudioSettings>('get_studio_settings');
      const base = structuredClone(DEFAULT_STUDIO);
      this.settings.set({
        ...base,
        ...remote,
        policy: {
          ...base.policy,
          ...remote.policy,
          categories: { ...base.policy.categories, ...remote.policy?.categories },
        },
      });
      this.error.set(null);
    } catch (e) {
      this.error.set(String(e));
    }
  }

  private async pushToDesktop(settings: StudioSettings) {
    if (!inDesktopShell()) return;
    try {
      await invoke('set_studio_settings', { settings });
      this.error.set(null);
    } catch (e) {
      this.error.set(String(e));
    }
  }

  private async refreshPack() {
    const platform = this.settings().platform;
    if (platform === 'generic') {
      this.pack.set('No shipped pack.');
      return;
    }
    try {
      const res = await fetch(`assets/platform-rules/${platform}.txt`);
      this.pack.set(res.ok ? (await res.text()).trim() : 'Platform pack could not be loaded.');
    } catch {
      this.pack.set('Platform pack could not be loaded.');
    }
  }
}

function ruleKey(platform: 'twitch' | 'youtube' | 'kick'): 'rules_twitch' | 'rules_youtube' | 'rules_kick' {
  if (platform === 'youtube') return 'rules_youtube';
  if (platform === 'kick') return 'rules_kick';
  return 'rules_twitch';
}

function loadStudio(): StudioSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return structuredClone(DEFAULT_STUDIO);
    const parsed = JSON.parse(raw) as Partial<StudioSettings>;
    const base = structuredClone(DEFAULT_STUDIO);
    return {
      ...base,
      ...parsed,
      rules_twitch: parsed.rules_twitch ?? base.rules_twitch,
      rules_youtube: parsed.rules_youtube ?? base.rules_youtube,
      rules_kick: parsed.rules_kick ?? base.rules_kick,
      private_terms: parsed.private_terms ?? base.private_terms,
      cloud_judge_consent: parsed.cloud_judge_consent ?? false,
      shortcut_veto: parsed.shortcut_veto || base.shortcut_veto,
      shortcut_approve: parsed.shortcut_approve || base.shortcut_approve,
      shortcut_dump: parsed.shortcut_dump || base.shortcut_dump,
      shortcut_safe: parsed.shortcut_safe || base.shortcut_safe,
      policy: {
        ...base.policy,
        ...parsed.policy,
        categories: { ...base.policy.categories, ...parsed.policy?.categories },
      },
    };
  } catch {
    return structuredClone(DEFAULT_STUDIO);
  }
}
