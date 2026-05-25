// Lightweight toast / notification system.
//
// Replaces window.alert / window.confirm pop-ups for transient
// confirmations like "Added 12 tracks to queue", "Playlist created",
// or "Could not connect to Discord". Errors set on `app.lastError`
// keep their dedicated full-width banner; this store handles
// success-and-info messages.

export type ToastKind = "info" | "success" | "warn" | "error";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
  /** When set, calling `dismiss(id)` for an unrelated id won't kill
   *  this one even if Svelte re-runs effects. */
  expiresAt: number;
}

const DEFAULT_DURATION = 3500;

class ToastStore {
  items = $state<Toast[]>([]);
  private nextId = 1;

  push(message: string, kind: ToastKind = "info", durationMs = DEFAULT_DURATION): number {
    const id = this.nextId++;
    const toast: Toast = {
      id,
      kind,
      message,
      expiresAt: Date.now() + durationMs,
    };
    this.items = [...this.items, toast];
    if (durationMs > 0) {
      window.setTimeout(() => this.dismiss(id), durationMs + 100);
    }
    return id;
  }

  /** Convenience helpers so call sites read naturally. */
  info(message: string, durationMs?: number): number {
    return this.push(message, "info", durationMs);
  }
  success(message: string, durationMs?: number): number {
    return this.push(message, "success", durationMs);
  }
  warn(message: string, durationMs?: number): number {
    return this.push(message, "warn", durationMs);
  }
  error(message: string, durationMs = 6000): number {
    return this.push(message, "error", durationMs);
  }

  dismiss(id: number): void {
    this.items = this.items.filter((t) => t.id !== id);
  }

  clear(): void {
    this.items = [];
  }
}

export const toasts = new ToastStore();
