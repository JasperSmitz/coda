// The only place that talks to the backend: typed intents, the startup
// snapshot and event subscriptions.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Intent, Progress, Snapshot, VisualizerFrame } from "./types";

/** Sends an intent to the engine. Failures (only while quitting) are logged. */
export function intent(value: Intent): void {
  invoke("intent", { intent: value }).catch((error: unknown) => {
    console.warn("intent failed", value.type, error);
  });
}

/** The full current state, queue included. */
export function fetchSnapshot(): Promise<Snapshot> {
  return invoke<Snapshot>("snapshot");
}

export function onSnapshot(handler: (snapshot: Snapshot) => void): Promise<UnlistenFn> {
  return listen<Snapshot>("snapshot", (event) => handler(event.payload));
}

export function onProgress(handler: (progress: Progress) => void): Promise<UnlistenFn> {
  return listen<Progress>("progress", (event) => handler(event.payload));
}

export function onVisualizer(handler: (frame: VisualizerFrame) => void): Promise<UnlistenFn> {
  return listen<VisualizerFrame>("visualizer", (event) => handler(event.payload));
}

export type CoverSize = "small" | "large";

/** The image URL for a track's cover at `size`. */
export function coverSrc(coverUrl: string, size: CoverSize): string {
  return `${coverUrl}/${size}`;
}
