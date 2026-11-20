// An album: a header with the large cover, "Album · year", the title, the
// artist (a link), "N songs · length" with its genres (links to their
// pages), Play / Shuffle and "…" (Add to playlist…, Add to mix…), then
// the track table grouped by disc.

import { useEffect } from "react";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { TrackTable } from "../components/TrackTable";
import { useLibraryVersion, usePageScroller, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { albumContext, openRoute, playTracks, useShuffleContext } from "../lib/play";
import { resolveGenreLink } from "../lib/genres";
import { addToMixFlow } from "../lib/mixes";
import { albumIngredient } from "../lib/mixRecipe";
import { addToPlaylistFlow, canAddToPlaylist } from "../lib/playlists";
import type { Album, Genre, SourceKind } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { usePageSubject } from "../state/pageSubject";
import { useAppState } from "../state/store";
import { PageActions, PageHeader, songsSummary } from "./common";

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
  const scroller = usePageScroller();
  const { snapshot } = useAppState();
  const started = snapshot?.phase === "ready";
  const server = snapshot !== null && snapshot.navidrome.state !== "notConfigured";
  const tracks = load.data?.tracks ?? [];
  useScroll(scroller, entry, tracks.length > 0, load.state !== "loading");
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
  usePageSubject(entry, album ? album.coverUrl : load.state === "loading" ? undefined : null, {
    glow: false,
  });

  // The album artist exactly as listed (Navidrome: with the album's id).
  const ingredient = album ? albumIngredient(source, { ...album, id }) : undefined;
  const songs = load.data ? tracks.length : (album?.trackCount ?? null);
  const length = album?.durationMs ?? null;

  return (
    <section className="view">
      <PageHeader
        bloom={album?.coverUrl ?? null}
        cover={
          <Cover
            url={album?.coverUrl ?? null}
            name={title}
            size="large"
            className="page-cover"
          />
        }
        eyebrow={album?.year != null ? `Album · ${album.year}` : "Album"}
        title={title}
        byline={
          artistName &&
          (artistId ? (
            <button
              className="link-button"
              onClick={() => nav.push({ kind: "artist", source, id: artistId, name: artistName })}
            >
              {artistName}
            </button>
          ) : (
            <span>{artistName}</span>
          ))
        }
        meta={
          <>
            <span>{songsSummary(songs, length)}</span>
            {genres.map((genre) => (
              <span key={genre.id} className="page-meta-item">
                <span className="page-meta-sep" aria-hidden="true">
                  ·
                </span>
                <button
                  className="genre-link"
                  title={genre.name}
                  onClick={() => openGenreLink(source, genre, entry)}
                >
                  {genre.name}
                </button>
              </span>
            ))}
          </>
        }
        actions={
          <PageActions
            onPlay={() => playTracks(tracks, undefined, false, context)}
            onShuffle={() => playTracks(tracks, undefined, true, context)}
            disabled={tracks.length === 0}
            menu={[
              ...(source === "navidrome"
                ? [
                    {
                      label: "Add to playlist…",
                      disabled: !started || !canAddToPlaylist(tracks, server),
                      action: () => void addToPlaylistFlow(tracks),
                    },
                  ]
                : []),
              {
                label: "Add to mix…",
                disabled: !started || !ingredient,
                hint:
                  ingredient === null
                    ? "An album without an artist cannot be in a mix"
                    : undefined,
                action: () => {
                  if (ingredient) void addToMixFlow(source, ingredient);
                },
              },
            ]}
          />
        }
      />
      <div className="view-body section-rule">
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
