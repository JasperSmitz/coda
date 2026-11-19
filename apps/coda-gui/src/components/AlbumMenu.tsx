// An album's menu (a grid tile's or a list row's "…", right-click or the
// context-menu key): Play, Shuffle, Play next, Add to queue, Add to mix…,
// Open album, Go to artist. `playAlbum` plays one from a tile or row
// (its round Play button, Shift+Enter).

import { browse } from "../lib/browse";
import { addToMixFlow } from "../lib/mixes";
import { albumIngredient } from "../lib/mixRecipe";
import { albumContext, enqueue, playFrom, playTracks } from "../lib/play";
import type { Album, SourceKind } from "../lib/types";
import { nav } from "../state/nav";
import { useAppState } from "../state/store";
import { ContextMenu, type MenuState } from "./ContextMenu";

export type AlbumMenuState = MenuState & { album: Album };

export function openAlbum(source: SourceKind, album: Album) {
  nav.push({ kind: "album", source, id: album.id, name: album.name, hint: album });
}

const tracksOf = (source: SourceKind, album: Album) =>
  browse.album(source, album.id).then((d) => d.tracks);

/** Plays (or shuffles) a whole album, named as its context. */
export function playAlbum(source: SourceKind, album: Album, shuffle = false) {
  playFrom(async () =>
    playTracks(await tracksOf(source, album), undefined, shuffle, albumContext(source, album)),
  );
}

/** Whether albums can be played now, and whether the queue can be edited. */
export function usePlayable(): { locked: boolean; disabled: boolean } {
  const { snapshot } = useAppState();
  // `locked`: no queue edits (starting or quitting); `disabled`: no playback either.
  const locked = snapshot?.phase !== "ready";
  const disabled = locked || snapshot.status === "unavailable";
  return { locked, disabled };
}

export function AlbumMenu({
  menu,
  source,
  onClose,
}: {
  menu: AlbumMenuState;
  source: SourceKind;
  onClose: () => void;
}) {
  const { locked, disabled } = usePlayable();
  const { album } = menu;
  return (
    <ContextMenu
      state={menu}
      onClose={onClose}
      items={[
        { label: "Play", disabled, action: () => playAlbum(source, album) },
        { label: "Shuffle", disabled, action: () => playAlbum(source, album, true) },
        {
          label: "Play next",
          disabled: locked,
          action: () => playFrom(async () => enqueue(await tracksOf(source, album), true)),
        },
        {
          label: "Add to queue",
          disabled: locked,
          action: () => playFrom(async () => enqueue(await tracksOf(source, album), false)),
        },
        // ADD TO PLAYLIST: "Add to playlist…" goes here (later slice).
        {
          label: "Add to mix…",
          // An album without an artist cannot be found again by name.
          disabled: locked || albumIngredient(source, album) === null,
          action: () => {
            const ingredient = albumIngredient(source, album);
            if (ingredient) void addToMixFlow(source, ingredient);
          },
        },
        { label: "Open album", action: () => openAlbum(source, album) },
        {
          label: "Go to artist",
          disabled: album.artistId === null,
          action: () =>
            album.artistId !== null &&
            nav.push({
              kind: "artist",
              source,
              id: album.artistId,
              name: album.artist ?? "Artist",
            }),
        },
      ]}
    />
  );
}
