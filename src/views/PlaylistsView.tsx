// Playlists of a source (local `.m3u` files or the server's), and one
// playlist's tracks in order, duplicates kept. Navidrome playlists the
// user owns can be created, renamed and deleted here, and their tracks
// removed and reordered (M3, `lib/playlists.ts`); read-only ones show
// why instead.

import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { ContextMenu, type MenuState } from "../components/ContextMenu";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { PlaylistTracks } from "../components/PlaylistTracks";
import { TrackTable } from "../components/TrackTable";
import { AlertIcon, MoreIcon } from "../components/icons";
import { useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { isRemoveKey } from "../lib/keys";
import { playTracks, useShuffleContext } from "../lib/play";
import {
  createPlaylistFlow,
  deletePlaylistFlow,
  renamePlaylistFlow,
  useEditsRunning,
  usePlaylistsVersion,
} from "../lib/playlists";
import type { Playlist, SourceKind } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { useAppState } from "../state/store";
import { PlayButtons, ScanLine, SOURCE_LABEL, summary, ViewHeader, plural } from "./common";

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
  const scroller = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const playlists = load.data ?? [];
  useScroll(scroller, entry, playlists.length > 0);
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
      <ViewHeader
        eyebrow={SOURCE_LABEL[source]}
        title="Playlists"
        actions={
          canEdit && (
            <button className="button" onClick={() => void createPlaylistFlow()}>
              New playlist…
            </button>
          )
        }
      >
        {load.state === "ready" && (
          <div className="secondary">{plural(playlists.length, "playlist")}</div>
        )}
      </ViewHeader>
      {source === "local" && <ScanLine />}
      <div className="view-body" ref={scroller}>
        {load.state === "loading" && <SkeletonRows />}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {load.state === "ready" && playlists.length === 0 && (
          <Empty title="No playlists">
            {source === "local"
              ? "No .m3u playlists were found in the music folder."
              : "The server has no playlists."}
          </Empty>
        )}
        {playlists.length > 0 && (
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
            list.current?.focus();
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
  const scroller = useRef<HTMLDivElement>(null);
  const detail = load.data;
  const tracks = detail?.tracks ?? [];
  useScroll(scroller, entry, tracks.length > 0);
  useShuffleContext(tracks.length > 0 ? () => playTracks(tracks, undefined, true) : null);

  const title = renamed ?? (detail?.playlist.name || hint?.name || name);
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
  const cover = hint?.coverUrl ?? detail?.playlist.coverUrl ?? tracks[0]?.coverUrl ?? null;
  return (
    <section className="view">
      <ViewHeader
        eyebrow={`${SOURCE_LABEL[source]} · Playlist`}
        title={title}
        cover={
          <Cover
            url={cover}
            name={title}
            size="large"
            className="header-cover album-header-cover"
          />
        }
        actions={
          <>
            <PlayButtons
              onPlay={() => playTracks(tracks)}
              onShuffle={() => playTracks(tracks, undefined, true)}
              disabled={tracks.length === 0}
            />
            {editable && (
              <div className="play-buttons">
                <button className="button" onClick={() => void rename()} disabled={busy}>
                  Rename…
                </button>
                <button className="button" onClick={() => void remove()} disabled={busy}>
                  Delete…
                </button>
              </div>
            )}
          </>
        }
      >
        <div className="album-meta">
          {(detail?.playlist.owner ?? hint?.owner) && (
            <span className="secondary">by {detail?.playlist.owner ?? hint?.owner}</span>
          )}
          <span className="secondary">
            {summary(
              detail ? tracks.length : (hint?.trackCount ?? null),
              detail?.playlist.durationMs ?? hint?.durationMs ?? null,
            )}
          </span>
        </div>
      </ViewHeader>
      <div className="view-body" ref={scroller}>
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
          />
        ) : (
          tracks.length > 0 && (
            <TrackTable
              tracks={tracks}
              scroller={scroller}
              showAlbum
              showArtist
              label={`Tracks of ${title}`}
            />
          )
        )}
      </div>
    </section>
  );
}
