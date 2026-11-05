// Playlists of a source (local `.m3u` files or the server's), and one
// playlist's tracks in order, duplicates kept.

import { useRef, useState, type KeyboardEvent } from "react";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { TrackTable } from "../components/TrackTable";
import { AlertIcon } from "../components/icons";
import { useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { playTracks, useShuffleContext } from "../lib/play";
import type { Playlist, SourceKind } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { PlayButtons, ScanLine, SOURCE_LABEL, summary, ViewHeader, plural } from "./common";

function openPlaylist(source: SourceKind, playlist: Playlist) {
  nav.push({ kind: "playlist", source, id: playlist.id, name: playlist.name, hint: playlist });
}

export function PlaylistsView({ entry, source }: { entry: Entry; source: SourceKind }) {
  const version = useLibraryVersion();
  const load = useLoad(() => browse.playlists(source), source, source === "local" ? version : null);
  const scroller = useRef<HTMLDivElement>(null);
  const playlists = load.data ?? [];
  useScroll(scroller, entry, playlists.length > 0);
  const [selected, setSelected] = useState<number | null>(null);

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
    } else return;
    e.preventDefault();
    e.stopPropagation();
    const index = Math.min(playlists.length - 1, Math.max(0, to));
    setSelected(index);
    document.getElementById(`playlist-${index}`)?.scrollIntoView({ block: "nearest" });
  };

  return (
    <section className="view">
      <ViewHeader eyebrow={SOURCE_LABEL[source]} title="Playlists">
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
            role="listbox"
            aria-label="Playlists"
            tabIndex={0}
            aria-activedescendant={selected !== null ? `playlist-${selected}` : undefined}
            onKeyDown={onKeyDown}
          >
            {playlists.map((playlist, index) => (
              <div
                key={playlist.id}
                id={`playlist-${index}`}
                role="option"
                aria-selected={selected === index}
                className={`playlist-row ${selected === index ? "is-selected" : ""}`}
                onClick={() => {
                  setSelected(index);
                  openPlaylist(source, playlist);
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
                {playlist.readonly && <span className="tag">Read-only</span>}
              </div>
            ))}
          </div>
        )}
      </div>
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
  const load = useLoad(
    () => browse.playlist(source, id),
    `${source}:${id}`,
    source === "local" ? version : null,
  );
  const scroller = useRef<HTMLDivElement>(null);
  const detail = load.data;
  const tracks = detail?.tracks ?? [];
  useScroll(scroller, entry, tracks.length > 0);
  useShuffleContext(tracks.length > 0 ? () => playTracks(tracks, undefined, true) : null);

  const title = detail?.playlist.name || hint?.name || name;
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
          <PlayButtons
            onPlay={() => playTracks(tracks)}
            onShuffle={() => playTracks(tracks, undefined, true)}
            disabled={tracks.length === 0}
          />
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
        {load.state === "loading" && <SkeletonRows />}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {load.state === "ready" && tracks.length === 0 && <Empty title="This playlist is empty" />}
        {tracks.length > 0 && (
          <TrackTable
            tracks={tracks}
            scroller={scroller}
            showAlbum
            showArtist
            label={`Tracks of ${title}`}
          />
        )}
      </div>
    </section>
  );
}
