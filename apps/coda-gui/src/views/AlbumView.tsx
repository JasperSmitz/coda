// An album: a header with the large cover, year, track count and length,
// its genres (links to their pages), Play / Shuffle, and the track table
// grouped by disc.

import { useEffect, useRef } from "react";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { AddToMixButton } from "../components/MixParts";
import { TrackTable } from "../components/TrackTable";
import { useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { albumContext, openRoute, playTracks, useShuffleContext } from "../lib/play";
import { resolveGenreLink } from "../lib/genres";
import { albumIngredient } from "../lib/mixRecipe";
import type { Album, Genre, SourceKind } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { PlayButtons, SOURCE_LABEL, summary, ViewHeader } from "./common";

/**
 * Opens an album's genre. On Navidrome the chip is resolved against the
 * server's genre listing (cached; one request at most, shared by every
 * chip) for its exact value; unlisted, or if the listing fails, it opens
 * by its cleaned name.
 */
function openGenreLink(source: SourceKind, genre: Genre, from: Entry) {
  if (source === "local") {
    openRoute({ kind: "genre", source, id: genre.id, name: genre.name });
    return;
  }
  void browse
    .genres("navidrome")
    .catch((): Genre[] => [])
    .then((listing) => {
      // Moved on while the listing loaded: don't jump away.
      if (nav.top().id !== from.id) return;
      const link = resolveGenreLink(genre, listing);
      openRoute({ kind: "genre", source, id: link.id, name: link.name, hint: link.hint });
    });
}

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
  const genres = load.data?.genres ?? [];
  const hasGenres = genres.length > 0;
  // Warm the genre listing so a chip opens at once.
  useEffect(() => {
    if (source === "navidrome" && hasGenres) browse.genres("navidrome").catch(() => {});
  }, [source, hasGenres]);
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
          <>
            <PlayButtons
              onPlay={() => playTracks(tracks, undefined, false, context)}
              onShuffle={() => playTracks(tracks, undefined, true, context)}
              disabled={tracks.length === 0}
            />
            {/* The album artist exactly as listed (Navidrome: with the album's id). */}
            <AddToMixButton
              source={source}
              ingredient={album ? albumIngredient(source, { ...album, id }) : undefined}
            />
          </>
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
        {genres.length > 0 && (
          <ul className="genre-links" aria-label="Genres">
            {genres.map((genre) => (
              <li key={genre.id}>
                <button
                  className="tag genre-link"
                  title={genre.name}
                  onClick={() => openGenreLink(source, genre, entry)}
                >
                  {genre.name}
                </button>
              </li>
            ))}
          </ul>
        )}
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
