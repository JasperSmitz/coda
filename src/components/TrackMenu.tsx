// The track context menu (DESIGN.md): Play, Play next, Add to queue, Go
// to album, Go to artist.

import { canGoToAlbum, canGoToArtist, enqueue, goToAlbum, goToArtist } from "../lib/play";
import type { Track } from "../lib/types";
import { ContextMenu, type MenuItem, type MenuState } from "./ContextMenu";

interface Props {
  state: MenuState;
  track: Track;
  /** Plays the track in its context (the list it is shown in). */
  onPlay: () => void;
  /** No playback (Play). */
  disabled: boolean;
  /** No queue edits at all (starting or quitting). */
  locked: boolean;
  onClose: () => void;
}

export function TrackMenu({ state, track, onPlay, disabled, locked, onClose }: Props) {
  const items: MenuItem[] = [
    { label: "Play", disabled, action: onPlay },
    { label: "Play next", disabled: locked, action: () => enqueue([track], true) },
    { label: "Add to queue", disabled: locked, action: () => enqueue([track], false) },
    // ADD TO PLAYLIST: "Add to playlist…" goes here (later slice; see
    // docs/PLAN.md). Keep it after the queue actions, before navigation.
    { label: "Go to album", disabled: !canGoToAlbum(track), action: () => goToAlbum(track) },
    { label: "Go to artist", disabled: !canGoToArtist(track), action: () => goToArtist(track) },
  ];
  return <ContextMenu state={state} items={items} onClose={onClose} />;
}
