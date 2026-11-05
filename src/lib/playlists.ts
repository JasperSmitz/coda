// Navidrome playlist edits (src-tauri/src/playlists/commands.rs) and the
// flows around them (name prompts, the delete and hidden-entries
// confirmations, the "Add to playlist" picker). The backend runs one edit
// at a time and shows each outcome as a notice; after every edit the
// cached playlist answers are forgotten and `usePlaylistsVersion` changes,
// so the shown playlist views reload.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, pickPlaylist, promptText } from "../state/dialogs";
import { forgetCached } from "./browse";
import { errorText } from "./ipc";
import type { EditOutcome, Hidden, Playlist, Track } from "./types";

// ----- reload signal ----------------------------------------------------

let version = 0;
let running = 0;
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((l) => l());
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
};

/** Changes after every playlist edit (a `useLoad` refresh value). */
export function usePlaylistsVersion(): number {
  return useSyncExternalStore(subscribe, () => version);
}

/** Edits sent and not answered yet. */
export function useEditsRunning(): number {
  return useSyncExternalStore(subscribe, () => running);
}

async function run(
  command: string,
  args: Record<string, unknown>,
  playlistId: string | null,
): Promise<EditOutcome> {
  running += 1;
  notify();
  let outcome: EditOutcome;
  try {
    outcome = await invoke<EditOutcome>(command, args);
  } catch (error) {
    outcome = { status: "failed", message: errorText(error), created: null };
  }
  running -= 1;
  // Partial and stale results changed (or showed a change to) the
  // playlist too: reload in every case but a refusal.
  if (outcome.status !== "refused") {
    forgetCached("playlists", ...(playlistId ? [`playlist:${playlistId}`] : []));
    version += 1;
  }
  notify();
  return outcome;
}

// ----- commands -----------------------------------------------------------

export const playlistEdits = {
  create: (name: string, songs: string[]) => run("playlist_create", { name, songs }, null),
  saveQueue: (name: string) => run("playlist_save_queue", { name }, null),
  add: (playlist: Playlist, songs: string[]) =>
    run("playlist_add", { playlistId: playlist.id, name: playlist.name, songs }, playlist.id),
  rename: (playlist: Playlist, name: string) =>
    run(
      "playlist_rename",
      { playlistId: playlist.id, oldName: playlist.name, name },
      playlist.id,
    ),
  delete: (playlist: Playlist) =>
    run("playlist_delete", { playlistId: playlist.id, name: playlist.name }, playlist.id),
  /** Removes `indices` of `expected` (the song ids as shown). */
  remove: (playlist: Playlist, expected: string[], indices: number[], hidden: Hidden) =>
    run(
      "playlist_remove",
      { playlistId: playlist.id, name: playlist.name, expected, indices, hidden },
      playlist.id,
    ),
  /** Saves `order`, a permutation of `expected`'s indices. */
  reorder: (playlist: Playlist, expected: string[], order: number[], hidden: Hidden) =>
    run(
      "playlist_reorder",
      { playlistId: playlist.id, name: playlist.name, expected, order, hidden },
      playlist.id,
    ),
};

// ----- helpers ------------------------------------------------------------

/** The Navidrome song ids of `tracks`, in order (local tracks have none). */
export function songIds(tracks: Track[]): string[] {
  return tracks.flatMap((t) => (t.source.kind === "navidrome" ? [t.source.songId] : []));
}

/** Coda's suggested name for a saved queue: `Queue 2026-11-05` (local date). */
export function queuePlaylistName(now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `Queue ${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/** What a rewrite of a playlist with `hidden` entries may also remove (Coda's `hidden_warning`). */
export function hiddenWarning(hidden: Hidden): string {
  const why = "(missing files or other libraries)";
  if (hidden.kind === "some") {
    return hidden.count === 1
      ? `This also removes 1 entry Navidrome hides ${why}.`
      : `This also removes ${hidden.count.toLocaleString()} entries Navidrome hides ${why}.`;
  }
  return `This may also remove entries Navidrome hides ${why}.`;
}

/**
 * Asks before a change that rewrites a playlist with hidden entries
 * (removing or reordering); `true` at once when there are none.
 */
export function confirmRewrite(hidden: Hidden, action: string): Promise<boolean> {
  if (hidden.kind === "none") return Promise.resolve(true);
  return confirm({
    title: `${action}?`,
    message: `Navidrome stores entries of this playlist it does not show. Saving the change replaces the whole list. ${hiddenWarning(hidden)}`,
    confirmLabel: action,
    danger: true,
  });
}

// ----- flows --------------------------------------------------------------

/** "New playlist…": asks for a name, then creates it with `songs`. */
export async function createPlaylistFlow(songs: string[] = []): Promise<EditOutcome | null> {
  const name = await promptText({
    title: "New playlist",
    label: "Name",
    initial: "",
    confirmLabel: "Create",
    note: songs.length > 0 ? `${songs.length.toLocaleString()} ${songs.length === 1 ? "song" : "songs"}` : undefined,
  });
  if (name === null || name.trim() === "") return null;
  return playlistEdits.create(name, songs);
}

/** "Save queue as playlist": asks for a name (Coda's default), then saves. */
export async function saveQueueFlow(remote: number, local: number): Promise<void> {
  const left = local === 0 ? undefined : `${local.toLocaleString()} local ${local === 1 ? "track is" : "tracks are"} left out.`;
  const name = await promptText({
    title: "Save queue as playlist",
    label: "Name",
    initial: queuePlaylistName(),
    confirmLabel: "Save",
    note: [`${remote.toLocaleString()} Navidrome ${remote === 1 ? "song" : "songs"}.`, left]
      .filter(Boolean)
      .join(" "),
  });
  if (name === null || name.trim() === "") return;
  await playlistEdits.saveQueue(name);
}

export async function renamePlaylistFlow(playlist: Playlist): Promise<EditOutcome | null> {
  const name = await promptText({
    title: "Rename playlist",
    label: "Name",
    initial: playlist.name,
    confirmLabel: "Rename",
  });
  if (name === null || name.trim() === "" || name.trim() === playlist.name) return null;
  return playlistEdits.rename(playlist, name);
}

export async function deletePlaylistFlow(playlist: Playlist): Promise<EditOutcome | null> {
  const ok = await confirm({
    title: "Delete playlist?",
    message: `“${playlist.name}” is deleted on the Navidrome server. This cannot be undone.`,
    confirmLabel: "Delete",
    danger: true,
  });
  return ok ? playlistEdits.delete(playlist) : null;
}

/** "Add to playlist…": the picker, then an add or a new playlist. */
export async function addToPlaylistFlow(tracks: Track[]): Promise<void> {
  const songs = songIds(tracks);
  if (songs.length === 0) return;
  const choice = await pickPlaylist(
    songs.length === 1 ? "Add to playlist" : `Add ${songs.length.toLocaleString()} songs to playlist`,
  );
  if (choice === null) return;
  if (choice === "new") await createPlaylistFlow(songs);
  else await playlistEdits.add(choice, songs);
}

/** Navidrome tracks can go to a playlist (and a server must be set up). */
export function canAddToPlaylist(tracks: Track[], serverConfigured: boolean): boolean {
  return serverConfigured && tracks.some((t) => t.source.kind === "navidrome");
}
