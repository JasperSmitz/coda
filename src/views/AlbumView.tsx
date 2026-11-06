// An album: a header with the large cover, year, track count and length,
// Play / Shuffle, and the track table grouped by disc.

import { useRef } from "react";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { TrackTable } from "../components/TrackTable";
import { useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { albumContext, playTracks, useShuffleContext } from "../lib/play";
import type { Album, SourceKind } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { PlayButtons, SOURCE_LABEL, summary, ViewHeader } from "./common";

interface Props {
  entry: Entry;
  source: SourceKind;
  id: string;
  name: string;
  hint?: Album;
}

export function AlbumView({ entry, source, id, name, hint }: Props) {
  const version = useLibraryVersion();
  const load = useLoad(
    () => browse.album(source, id),
    `${source}:${id}`,
    source === "local" ? version : null,
  );
  const scroller = useRef<HTMLDivElement>(null);
  const tracks = load.data?.tracks ?? [];
  useScroll(scroller, entry, tracks.length > 0);
  // The listing's data first (it knows the year), the album's own after.
  const loaded = load.data?.album;
  const album: Album | undefined = loaded
    ? {
        ...loaded,
        year: loaded.year ?? hint?.year ?? null,
        artistId: loaded.artistId ?? hint?.artistId ?? null,
      }
    : hint;
  const title = album?.name ?? name;
  const mixed = new Set(tracks.map((t) => t.artist)).size > 1;
  const artistId = album?.artistId ?? null;
  const artistName = album?.artist ?? null;
  const context = albumContext(source, {
    id,
    name: title,
    artist: artistName,
    coverUrl: album?.coverUrl ?? null,
  });
  useShuffleContext(tracks.length > 0 ? () => playTracks(tracks, undefined, true, context) : null);

  return (
    <section className="view">
      <ViewHeader
        eyebrow={`${SOURCE_LABEL[source]} · Album`}
        title={title}
        cover={
          <Cover
            url={album?.coverUrl ?? null}
            name={title}
            size="large"
            className="header-cover album-header-cover"
          />
        }
        actions={
          <PlayButtons
            onPlay={() => playTracks(tracks, undefined, false, context)}
            onShuffle={() => playTracks(tracks, undefined, true, context)}
            disabled={tracks.length === 0}
          />
        }
      >
        <div className="album-meta">
          {artistName &&
            (artistId ? (
              <button
                className="link-button"
                onClick={() => nav.push({ kind: "artist", source, id: artistId, name: artistName })}
              >
                {artistName}
              </button>
            ) : (
              <span>{artistName}</span>
            ))}
          {album?.year != null && <span className="secondary">{album.year}</span>}
          <span className="secondary">
            {summary(
              load.data ? tracks.length : (album?.trackCount ?? null),
              album?.durationMs ?? null,
            )}
          </span>
        </div>
      </ViewHeader>
      <div className="view-body" ref={scroller}>
        {load.state === "loading" && <SkeletonRows />}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {load.state === "ready" && tracks.length === 0 && <Empty title="No tracks" />}
        {tracks.length > 0 && (
          <TrackTable
            tracks={tracks}
            scroller={scroller}
            showArtist={mixed}
            discs
            label={`Tracks of ${title}`}
            context={context}
          />
        )}
      </div>
    </section>
  );
}
