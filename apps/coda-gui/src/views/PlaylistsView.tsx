// Playlists of a source (local `.m3u` files or the server's) as cards
// (Covers: a 2×2 mosaic of its albums' covers, or its own cover; the
// name and song count) or rows (List), and one playlist's tracks in
// order, duplicates kept. Navidrome playlists the
// user owns can be created, renamed and deleted here, and their tracks
// removed and reordered (M3, `lib/playlists.ts`); read-only ones show
// why instead.

import { memo, useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { gridKeyTarget } from "../components/AlbumGrid";
import { ContextMenu, type MenuState } from "../components/ContextMenu";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { PlaylistTracks } from "../components/PlaylistTracks";
import { TrackTable } from "../components/TrackTable";
import { AlertIcon, MoreIcon } from "../components/icons";
import { useLibraryVersion, usePageScroller, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { isRemoveKey } from "../lib/keys";
import { morphKey } from "../lib/pageTransition";
import { playlistContext, playTracks, useShuffleContext } from "../lib/play";
import {
  createPlaylistFlow,
  deletePlaylistFlow,
  renamePlaylistFlow,
  useEditsRunning,
  usePlaylistsVersion,
} from "../lib/playlists";
import type { Playlist, SourceKind } from "../lib/types";
import { useLibraryView } from "../state/libraryView";
import { nav, type Entry } from "../state/nav";
import { usePageSubject } from "../state/pageSubject";
import { useAppState } from "../state/store";
import {
  CoverMosaic,
  ListHeader,
  PageActions,
  PageHeader,
  ScanLine,
  SOURCE_LABEL,
  ViewToggle,
  songsSummary,
  summary,
  plural,
} from "./common";

/** Shown instead of index-based edits when not every entry could be read (Coda's). */
const INCOMPLETE = "This playlist has entries Coda cannot show; edit it in Navidrome";

/** Playlist edits are possible (started; edits wait for each other in the backend). */
function useCanEdit(): boolean {
  const { snapshot } = useAppState();
  return snapshot?.phase === "ready" && snapshot.navidrome.state !== "notConfigured";
}

function openPlaylist(source: SourceKind, playlist: Playlist) {
  nav.push({ kind: "playlist", source, id: playlist.id, name: playlist.name, hint: playlist });
}

export function PlaylistsView({ entry, source }: { entry: Entry; source: SourceKind }) {
  const version = useLibraryVersion();
  const edits = usePlaylistsVersion();
  const load = useLoad(() => browse.playlists(source), source, source === "local" ? version : edits);
  const scroller = usePageScroller();
  const list = useRef<HTMLDivElement>(null);
  const playlists = load.data ?? [];
  const [view, setView, known] = useLibraryView(source, "playlists");
  useScroll(scroller, entry, playlists.length > 0 && known, load.state !== "loading");
  const idPrefix = useId();
  const [selected, setSelected] = useState<number | null>(null);
  const [menu, setMenu] = useState<(MenuState & { playlist: Playlist }) | null>(null);
  const canEdit = useCanEdit() && source === "navidrome";

  const rename = (playlist: Playlist) => {
    if (canEdit && playlist.editable) void renamePlaylistFlow(playlist);
  };
  const remove = (playlist: Playlist) => {
    if (canEdit && playlist.editable) void deletePlaylistFlow(playlist);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (playlists.length === 0) return;
    const at = selected ?? -1;
    let to: number;
    if (e.key === "ArrowDown") to = at + 1;
    else if (e.key === "ArrowUp") to = at - 1;
    else if (e.key === "Home") to = 0;
    else if (e.key === "End") to = playlists.length - 1;
    else if (e.key === "Enter" && selected !== null) {
      openPlaylist(source, playlists[selected]);
      e.preventDefault();
      return;
    } else if (e.key === "F2" && selected !== null) {
      rename(playlists[selected]);
      e.preventDefault();
      return;
    } else if (isRemoveKey(e) && selected !== null) {
      remove(playlists[selected]);
      e.preventDefault();
      e.stopPropagation();
      return;
    } else if (e.key === "ContextMenu" && selected !== null && canEdit) {
      const row = document.getElementById(`${idPrefix}-${selected}`)?.getBoundingClientRect();
      setMenu({ x: (row?.left ?? 0) + 40, y: row?.bottom ?? 0, key: playlists[selected].id, playlist: playlists[selected] });
      e.preventDefault();
      return;
    } else return;
    e.preventDefault();
    e.stopPropagation();
    const index = Math.min(playlists.length - 1, Math.max(0, to));
    setSelected(index);
    document.getElementById(`${idPrefix}-${index}`)?.scrollIntoView({ block: "nearest" });
  };

  return (
    <section className="view">
      <ListHeader
        eyebrow={SOURCE_LABEL[source]}
        title="Playlists"
        meta={load.state === "ready" && plural(playlists.length, "playlist")}
        tools={
          <>
            {canEdit && (
              <button className="button" onClick={() => void createPlaylistFlow()}>
                New playlist…
              </button>
            )}
            <ViewToggle value={view} onChange={setView} />
          </>
        }
      />
      {source === "local" && <ScanLine />}
      <div className="view-body">
        {(load.state === "loading" || (!known && playlists.length > 0)) && <SkeletonRows />}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {load.state === "ready" && playlists.length === 0 && (
          <Empty title="No playlists">
            {source === "local"
              ? "No .m3u playlists were found in the music folder."
              : "The server has no playlists."}
          </Empty>
        )}
        {playlists.length > 0 && known && view === "covers" && (
          <PlaylistCards
            playlists={playlists}
            source={source}
            canEdit={canEdit}
            onOpen={(playlist) => openPlaylist(source, playlist)}
            onRename={rename}
            onRemove={remove}
            onMenu={(playlist, x, y) => {
              if (canEdit) setMenu({ x, y, key: playlist.id, playlist });
            }}
          />
        )}
        {playlists.length > 0 && known && view === "list" && (
          <div
            className="playlist-list"
            ref={list}
            role="listbox"
            aria-label="Playlists"
            tabIndex={0}
            aria-activedescendant={selected !== null ? `${idPrefix}-${selected}` : undefined}
            onKeyDown={onKeyDown}
          >
            {playlists.map((playlist, index) => (
              <div
                key={playlist.id}
                id={`${idPrefix}-${index}`}
                role="option"
                aria-selected={selected === index}
                className={`playlist-row ${selected === index ? "is-selected" : ""}`}
                onClick={() => {
                  setSelected(index);
                  openPlaylist(source, playlist);
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setSelected(index);
                  if (canEdit) setMenu({ x: e.clientX, y: e.clientY, key: playlist.id, playlist });
                }}
              >
                <Cover
                  url={playlist.coverUrl}
                  name={playlist.name}
                  size="small"
                  className="playlist-thumb"
                />
                <span className="playlist-name">{playlist.name}</span>
                <span className="secondary">
                  {summary(playlist.trackCount, playlist.durationMs)}
                </span>
                {source === "navidrome" && !playlist.editable ? (
                  <span className="tag" title={playlist.readOnlyReason ?? undefined}>
                    Read-only
                  </span>
                ) : (
                  playlist.readonly && <span className="tag">Read-only</span>
                )}
                {canEdit && playlist.editable && (
                  <button
                    className="icon-button playlist-more"
                    tabIndex={-1}
                    aria-label={`More for ${playlist.name}`}
                    title="More"
                    onClick={(e) => {
                      e.stopPropagation();
                      const rect = e.currentTarget.getBoundingClientRect();
                      setSelected(index);
                      setMenu({ x: rect.left, y: rect.bottom, key: playlist.id, playlist });
                    }}
                  >
                    <MoreIcon size={16} />
                  </button>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
      {menu && (
        <ContextMenu
          state={menu}
          onClose={() => {
            setMenu(null);
            if (view === "list") {
              list.current?.focus();
              return;
            }
            // Covers: back to the playlist's card.
            const at = playlists.findIndex((p) => p.id === menu.playlist.id);
            document.querySelector<HTMLElement>(`.playlist-cards [data-index="${at}"]`)?.focus();
          }}
          items={[
            { label: "Open", action: () => openPlaylist(source, menu.playlist) },
            { label: "Rename…", disabled: !menu.playlist.editable, action: () => rename(menu.playlist) },
            { label: "Delete…", disabled: !menu.playlist.editable, action: () => remove(menu.playlist) },
          ]}
        />
      )}
    </section>
  );
}

/**
 * Playlists as cards (Covers): the arrow keys, Home and End move between
 * them, Enter opens one, F2 renames, Delete removes (editable ones), the
 * context-menu key (or Shift+F10) opens the menu.
 */
function PlaylistCards({
  playlists,
  source,
  canEdit,
  onOpen,
  onRename,
  onRemove,
  onMenu,
}: {
  playlists: Playlist[];
  source: SourceKind;
  canEdit: boolean;
  onOpen: (playlist: Playlist) => void;
  onRename: (playlist: Playlist) => void;
  onRemove: (playlist: Playlist) => void;
  onMenu: (playlist: Playlist, x: number, y: number) => void;
}) {
  const grid = useRef<HTMLDivElement>(null);
  const columns = () => {
    const cards = grid.current?.querySelectorAll<HTMLElement>(".playlist-card");
    if (!cards || cards.length === 0) return 1;
    const top = cards[0].offsetTop;
    let n = 0;
    while (n < cards.length && cards[n].offsetTop === top) n++;
    return Math.max(1, n);
  };
  const onKeyDown = (e: KeyboardEvent) => {
    const target = e.target as HTMLElement;
    const at = Number(target.dataset.index ?? -1);
    if (at < 0) return;
    const playlist = playlists[at];
    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      const rect = target.getBoundingClientRect();
      onMenu(playlist, rect.left + 24, rect.top + 40);
      e.preventDefault();
      return;
    }
    if (e.key === "F2") {
      onRename(playlist);
      e.preventDefault();
      return;
    }
    if (isRemoveKey(e)) {
      onRemove(playlist);
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    const to = gridKeyTarget(e.key, at, columns(), playlists.length);
    if (to === null) return;
    e.preventDefault();
    e.stopPropagation();
    const el = grid.current?.querySelector<HTMLElement>(`[data-index="${to}"]`);
    el?.focus({ preventScroll: true });
    el?.scrollIntoView({ block: "nearest" });
  };
  return (
    <div
      ref={grid}
      className="playlist-cards"
      role="list"
      aria-label="Playlists"
      onKeyDown={onKeyDown}
    >
      {playlists.map((playlist, index) => (
        <PlaylistCard
          key={playlist.id}
          playlist={playlist}
          index={index}
          source={source}
          canEdit={canEdit}
          onOpen={onOpen}
          onMenu={onMenu}
        />
      ))}
    </div>
  );
}

const PlaylistCard = memo(function PlaylistCard({
  playlist,
  index,
  source,
  canEdit,
  onOpen,
  onMenu,
}: {
  playlist: Playlist;
  index: number;
  source: SourceKind;
  canEdit: boolean;
  onOpen: (playlist: Playlist) => void;
  onMenu: (playlist: Playlist, x: number, y: number) => void;
}) {
  const readOnly = source === "navidrome" && !playlist.editable;
  const sub = [
    playlist.trackCount !== null ? plural(playlist.trackCount, "song") : null,
    readOnly || playlist.readonly ? "Read-only" : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <div role="listitem" className="playlist-card-item">
      <button
        className="playlist-card"
        data-index={index}
        title={playlist.name}
        aria-keyshortcuts={canEdit && playlist.editable ? "F2 Delete" : undefined}
        onClick={() => onOpen(playlist)}
        onContextMenu={(e) => {
          e.preventDefault();
          onMenu(playlist, e.clientX, e.clientY);
        }}
      >
        <PlaylistCover playlist={playlist} source={source} />
        <span className="playlist-card-name">{playlist.name}</span>
        <span className="playlist-card-sub secondary">{sub}</span>
      </button>
    </div>
  );
});

/** A playlist's cover on its card: its albums' mosaic (local), else its own (Navidrome's is a mosaic already). */
function PlaylistCover({ playlist, source }: { playlist: Playlist; source: SourceKind }) {
  const morph = morphKey("playlist", source, playlist.id);
  return playlist.covers.length >= 4 ? (
    <CoverMosaic
      urls={playlist.covers}
      name={playlist.name}
      size="small"
      className="playlist-card-cover"
      morph={morph}
    />
  ) : (
    <Cover
      url={playlist.coverUrl}
      name={playlist.name}
      size="small"
      className="playlist-card-cover"
      morph={morph}
    />
  );
}

interface PlaylistProps {
  entry: Entry;
  source: SourceKind;
  id: string;
  name: string;
  hint?: Playlist;
}

export function PlaylistView({ entry, source, id, name, hint }: PlaylistProps) {
  const version = useLibraryVersion();
  const edits = usePlaylistsVersion();
  const load = useLoad(
    () => browse.playlist(source, id),
    `${source}:${id}`,
    source === "local" ? version : edits,
  );
  const canEdit = useCanEdit() && source === "navidrome";
  const busy = useEditsRunning() > 0;
  // A rename shows at once; the listing's name is the fallback.
  const [renamed, setRenamed] = useState<string | null>(null);
  const scroller = usePageScroller();
  const detail = load.data;
  const tracks = detail?.tracks ?? [];
  useScroll(scroller, entry, tracks.length > 0, load.state !== "loading");
  const title = renamed ?? (detail?.playlist.name || hint?.name || name);
  // Its own cover, else (header) a mosaic of its first covers.
  const ownCover = hint?.coverUrl ?? detail?.playlist.coverUrl ?? null;
  // Local: its albums' covers, as on its card (so the card's cover morphs into this one).
  const mosaic = hint?.covers ?? detail?.playlist.covers ?? [];
  const cover = ownCover ?? tracks[0]?.coverUrl ?? null;
  const context = playlistContext(source, { id, name: title, coverUrl: cover });
  useShuffleContext(tracks.length > 0 ? () => playTracks(tracks, undefined, true, context) : null);
  usePageSubject(entry, cover ?? (load.state === "loading" ? undefined : null));

  // The detail has no name (`getPlaylist` sends none here): use the shown one.
  const playlist: Playlist | null = detail ? { ...detail.playlist, name: title } : null;
  const editable = canEdit && playlist !== null && playlist.editable;
  const complete = detail !== undefined && detail.missing === 0 && detail.truncated === null;
  const readOnly =
    source === "navidrome" && detail && !detail.playlist.editable
      ? detail.playlist.readOnlyReason
      : null;

  // The detail has no name: after any playlist edit (a rename here or in
  // the list), take the server's name from the fresh listing; a failed
  // listing keeps the shown one.
  useEffect(() => {
    if (source !== "navidrome" || edits === 0) return;
    let current = true;
    browse.playlists("navidrome").then(
      (list) => {
        const found = list.find((p) => p.id === id);
        if (current && found) setRenamed(found.name);
      },
      () => {},
    );
    return () => {
      current = false;
    };
  }, [edits, id, source]);
  const rename = async () => {
    if (!playlist) return;
    const result = await renamePlaylistFlow(playlist);
    // Shown at once; the listing above replaces it with the server's.
    if (result?.outcome.status === "done") setRenamed(result.name);
  };
  const remove = async () => {
    if (!playlist) return;
    const outcome = await deletePlaylistFlow(playlist);
    if (outcome?.status === "done") nav.back();
  };
  return (
    <section className="view">
      <PageHeader
        cover={
          mosaic.length >= 4 ? (
            <CoverMosaic urls={mosaic} name={title} />
          ) : ownCover ? (
            <Cover url={ownCover} name={title} size="large" className="page-cover" />
          ) : (
            <CoverMosaic urls={tracks.slice(0, 50).map((t) => t.coverUrl)} name={title} />
          )
        }
        eyebrow="Playlist"
        title={title}
        byline={
          (detail?.playlist.owner ?? hint?.owner) && (
            <span className="secondary">by {detail?.playlist.owner ?? hint?.owner}</span>
          )
        }
        meta={songsSummary(
          detail ? tracks.length : (hint?.trackCount ?? null),
          detail?.playlist.durationMs ?? hint?.durationMs ?? null,
        )}
        actions={
          <PageActions
            onPlay={() => playTracks(tracks, undefined, false, context)}
            onShuffle={() => playTracks(tracks, undefined, true, context)}
            disabled={tracks.length === 0}
            menu={
              editable
                ? [
                    { label: "Rename…", disabled: busy, action: () => void rename() },
                    { label: "Delete…", disabled: busy, action: () => void remove() },
                  ]
                : undefined
            }
          />
        }
      />
      <div className="view-body">
        {detail?.warning && (
          <div className="inline-warning" role="note">
            <AlertIcon size={16} />
            <span>{detail.warning}</span>
          </div>
        )}
        {readOnly && (
          <div className="inline-note secondary" role="note">
            {readOnly}
          </div>
        )}
        {editable && !complete && tracks.length > 0 && (
          <div className="inline-note secondary" role="note">
            {INCOMPLETE}
          </div>
        )}
        {editable && complete && tracks.length > 0 && (
          <div className="inline-note secondary" role="note">
            {busy
              ? "Saving…"
              : "Drag tracks (or Alt+↑/↓) to reorder; Delete or ⌘⌫ removes the selection (after a confirmation). Changes are saved at once."}
          </div>
        )}
        {load.state === "loading" && <SkeletonRows />}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {load.state === "ready" && tracks.length === 0 && <Empty title="This playlist is empty" />}
        {tracks.length > 0 && editable && complete && playlist && detail ? (
          <PlaylistTracks
            playlist={playlist}
            tracks={tracks}
            hidden={detail.hidden}
            scroller={scroller}
            label={`Tracks of ${title}`}
            context={context}
          />
        ) : (
          tracks.length > 0 && (
            <TrackTable
              tracks={tracks}
              scroller={scroller}
              showAlbum
              showArtist
              label={`Tracks of ${title}`}
              context={context}
            />
          )
        )}
      </div>
    </section>
  );
}
