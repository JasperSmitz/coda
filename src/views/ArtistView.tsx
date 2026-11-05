// An artist: their albums as a grid, with Play and Shuffle for all of
// the artist's tracks (`s` shuffles them too).

import { useRef } from "react";
import { AlbumGrid } from "../components/AlbumGrid";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonGrid } from "../components/LoadState";
import { useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { playTracks, useShuffleContext } from "../lib/play";
import type { SourceKind } from "../lib/types";
import type { Entry } from "../state/nav";
import { PlayButtons, SOURCE_LABEL, ViewHeader, plural } from "./common";

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
  const scroller = useRef<HTMLDivElement>(null);
  const albums = load.data?.albums ?? [];
  useScroll(scroller, entry, albums.length > 0);

  const play = (shuffle: boolean) =>
    void browse.artistTracks(source, id).then((tracks) => playTracks(tracks, undefined, shuffle));
  useShuffleContext(load.state === "ready" && albums.length > 0 ? () => play(true) : null);

  const artist = load.data?.artist;
  const title = artist?.name || name;
  const cover = artist?.coverUrl ?? albums[0]?.coverUrl ?? null;
  const tracks = artist?.trackCount;
  return (
    <section className="view">
      <ViewHeader
        eyebrow={`${SOURCE_LABEL[source]} · Artist`}
        title={title}
        cover={<Cover url={cover} name={title} size="small" className="header-cover round" />}
        actions={
          <PlayButtons
            onPlay={() => play(false)}
            onShuffle={() => play(true)}
            disabled={albums.length === 0}
          />
        }
      >
        <div className="secondary">
          {load.state === "ready" &&
            [plural(albums.length, "album"), tracks != null ? plural(tracks, "track") : null]
              .filter(Boolean)
              .join(" · ")}
        </div>
      </ViewHeader>
      <div className="view-body" ref={scroller}>
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
