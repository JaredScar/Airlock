import { Component, OnDestroy, signal } from '@angular/core';
import { DecimalPipe } from '@angular/common';
import { CATEGORY_LABEL, type Category } from './airlock.types';

interface HoldItem {
  id: number;
  category: string;
  status: string;
  mode: string;
  action: string;
  start_us: number;
  end_us: number;
  evidence: string[];
  reasons: string[];
}

@Component({
  selector: 'app-moderator',
  imports: [DecimalPipe],
  templateUrl: './moderator.component.html',
})
export class ModeratorComponent implements OnDestroy {
  readonly host = signal('127.0.0.1');
  readonly port = signal(1940);
  readonly password = signal('');
  readonly note = signal('Enter the stream PC address, the port, and the password.');
  readonly items = signal<HoldItem[]>([]);
  readonly playhead = signal(0);
  readonly connected = signal(false);
  private socket: WebSocket | null = null;

  ngOnDestroy() {
    this.disconnect();
  }

  label(category: string): string {
    if (category in CATEGORY_LABEL) return CATEGORY_LABEL[category as Category];
    return category;
  }

  secondsUntil(item: HoldItem): number {
    return Math.max(0, (item.start_us - this.playhead()) / 1_000_000);
  }

  /** Same rule as the on-air queue: a suggestion redacts only after approval. */
  active(item: HoldItem): boolean {
    if (item.action === 'log_only') return false;
    if (item.mode === 'apply') return item.status !== 'vetoed';
    if (item.mode === 'suggest') return item.status === 'approved';
    return false;
  }

  onHost(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.host.set(input.value);
  }

  onPort(event: Event) {
    const input = event.target;
    if (!(input instanceof HTMLInputElement)) return;
    const port = Number(input.value);
    if (Number.isFinite(port)) this.port.set(Math.min(65535, Math.max(1, Math.round(port))));
  }

  onPassword(event: Event) {
    const input = event.target;
    if (input instanceof HTMLInputElement) this.password.set(input.value);
  }

  connect() {
    this.disconnect();
    const host = this.host().trim();
    const port = this.port();
    if (!/^[A-Za-z0-9.:-]+$/.test(host) || port < 1 || port > 65535) {
      this.note.set('Enter the stream PC address and port.');
      return;
    }
    const password = this.password();
    let socket: WebSocket;
    try {
      socket = new WebSocket(`ws://${host}:${port}`);
    } catch {
      this.note.set('Could not open the moderator connection.');
      return;
    }
    this.socket = socket;
    socket.onopen = () => {
      socket.send(JSON.stringify({ op: 'auth', token: password }));
      this.connected.set(true);
      this.note.set('Connected. Waiting for the hold.');
    };
    socket.onmessage = (event) => this.onFrame(String(event.data));
    socket.onerror = () => {
      if (this.socket === socket) this.note.set('The moderator connection failed.');
    };
    socket.onclose = () => {
      if (this.socket === socket) {
        this.socket = null;
        this.connected.set(false);
      }
    };
  }

  disconnect() {
    const socket = this.socket;
    this.socket = null;
    this.connected.set(false);
    if (socket && socket.readyState < WebSocket.CLOSING) socket.close();
  }

  decide(op: 'approve' | 'veto', id: number) {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      this.note.set('Connect to the stream PC first.');
      return;
    }
    socket.send(JSON.stringify({ op, id }));
  }

  private onFrame(text: string) {
    let frame: { ok?: boolean; detail?: string; playhead_us?: number; items?: HoldItem[] };
    try {
      frame = JSON.parse(text) as typeof frame;
    } catch {
      this.note.set('The moderator connection sent something this page could not read.');
      return;
    }
    if (frame.ok === false) {
      this.note.set(frame.detail || 'The moderator connection was refused.');
      return;
    }
    this.playhead.set(frame.playhead_us ?? 0);
    this.items.set(Array.isArray(frame.items) ? frame.items : []);
    this.note.set(this.items().length === 0 ? 'Nothing is waiting in the hold.' : 'Choose approve or veto before it airs.');
  }
}
