// An artist: "Your most played" (up to six of their songs by this app's
// listens, hidden without any; Enter or double-click plays from that
// song in its album), then their albums as a grid, with Play and Shuffle
// for all of the artist's tracks (`s` shuffles them too).

import { AlbumGrid } from "../components/AlbumGrid";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonGrid } from "../components/LoadState";
import { TrackTable } from "../components/TrackTable";
import { useLibraryVersion, usePageScroller, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { addToMixFlow } from "../lib/mixes";
import { artistIngredient } from "../lib/mixRecipe";
import {
  albumContext,
  artistContext,
  playFrom,
  playLocal,
  playTracks,
  sameTrack,
  useShuffleContext,
} from "../lib/play";
import type { SourceKind, TopTrack } from "../lib/types";
import type { Entry } from "../state/nav";
import { usePageSubject } from "../state/pageSubject";
import { useAppState } from "../state/store";
import { PageActions, PageHeader, plural } from "./common";

interface Props {
  entry: Entry;
  source: SourceKind;
  id: string;
  name: string;
}

export function ArtistView({ entry, source, id, name }: Props) {
  const version = useLibraryVersion();
  const load = useLoad(
    () => browse.artist(source, id),
    `${source}:${id}`,
    source === "local" ? version : null,
  );
  const scroller = usePageScroller();
  const { snapshot } = useAppState();
  const started = snapshot?.phase === "ready";
  const albums = load.data?.albums ?? [];
  useScroll(scroller, entry, albums.length > 0, load.state !== "loading");

  const artist = load.data?.artist;
  const title = artist?.name || name;
  const cover = artist?.coverUrl ?? albums[0]?.coverUrl ?? null;
  usePageSubject(entry, cover);

  // Local: the engine resolves the artist (no track list over IPC) and
  // records it in the play history itself.
  const play = (shuffle: boolean) =>
    source === "local"
      ? playLocal({ kind: "artist", id }, shuffle)
      : playFrom(async () =>
          playTracks(
            await browse.navidromeArtistTracks(id),
            undefined,
            shuffle,
            artistContext(source, { id, name: title, coverUrl: cover }),
          ),
        );
  useShuffleContext(load.state === "ready" && albums.length > 0 ? () => play(true) : null);
  const tracks = artist?.trackCount;
  return (
    <section className="view">
      <PageHeader
        cover={<Cover url={cover} name={title} size="large" className="page-cover" />}
        round
        eyebrow="Artist"
        title={title}
        meta={
          load.state === "ready" &&
          [plural(albums.length, "album"), tracks != null ? plural(tracks, "song") : null]
            .filter(Boolean)
            .join(" · ")
        }
        actions={
          <PageActions
            onPlay={() => play(false)}
            onShuffle={() => play(true)}
            disabled={albums.length === 0}
            menu={[
              {
                label: "Add to mix…",
                disabled: !started,
                action: () =>
                  void addToMixFlow(source, artistIngredient(source, { id, name: title })),
              },
            ]}
          />
        }
      />
      <MostPlayed source={source} id={id} name={title} />
      <div className="view-body">
        {load.state === "loading" && <SkeletonGrid count={6} />}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {load.state === "ready" && albums.length === 0 && <Empty title="No albums" />}
        {albums.length > 0 && (
          <AlbumGrid
            albums={albums}
            source={source}
            scroller={scroller}
            hideArtist
            label={`Albums by ${title}`}
          />
        )}
      </div>
    </section>
  );
}

/**
 * Plays from `top`'s song in its album (the album's context); a song
 * whose album cannot be loaded, or is no longer on it, plays alone.
 */
function playInAlbum(source: SourceKind, top: TopTrack) {
  const { track } = top;
  const albumId = track.albumId;
  if (albumId === null) {
    playTracks([track]);
    return;
  }
  playFrom(async () => {
    const detail = await browse.album(source, albumId);
    const at = detail.tracks.findIndex((t) => sameTrack(t, track));
    if (at < 0) playTracks([track]);
    else playTracks(detail.tracks, at, false, albumContext(source, detail.album));
  });
}

/** "Your most played": the artist's songs by our listens, hidden without any. */
function MostPlayed({ source, id, name }: { source: SourceKind; id: string; name: string }) {
  const { snapshot } = useAppState();
  const version = useLibraryVersion();
  // Reloads quietly as listens are counted (the history's revision).
  const revision = snapshot?.historyRevision ?? 0;
  const load = useLoad(
    () => browse.artistMostPlayed(source, id, name),
    `${source}:${id}:${name}`,
    `${source === "local" ? version : ""}|${revision}`,
  );
  const scroller = usePageScroller();
  const top = load.data ?? [];
  if (top.length === 0) return null;
  return (
    <section className="artist-most-played" aria-labelledby="artist-most-played">
      <h2 id="artist-most-played" className="section-title">
        Your most played
      </h2>
      <TrackTable
        tracks={top.map((t) => t.track)}
        scroller={scroller}
        covers
        notes={top.map((t) => plural(t.listens, "play"))}
        onPlay={(index) => playInAlbum(source, top[index])}
        label="Your most played"
      />
    </section>
  );
}
