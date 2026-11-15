// Genres of a source, A–Z with album and track counts, and one genre's
// albums. Local: Play and Shuffle play every track of the genre (resolved
// by the engine). Navidrome: Shuffle only, random songs of the genre (as
// Shuffle all does); its albums are paged like the Albums view, in the
// server's order. `s` shuffles the genre on its page, and everything on
// the list.

import { useId, useRef, useState, type KeyboardEvent } from "react";
import { AlbumGrid } from "../components/AlbumGrid";
import { Empty, ErrorState, SkeletonGrid, SkeletonRows } from "../components/LoadState";
import { PagedAlbumsBody, usePagedAlbums } from "../components/PagedAlbums";
import { useElementSize, useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { playFrom, playLocal, playTracks, shuffleAll, useShuffleContext } from "../lib/play";
import type { Genre, SourceKind } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { showUiNotice } from "../state/uiNotices";
import { PlayButtons, ScanLine, SOURCE_LABEL, ViewHeader, plural } from "./common";

export function openGenre(source: SourceKind, genre: Pick<Genre, "id" | "name">, hint?: Genre) {
  nav.push({ kind: "genre", source, id: genre.id, name: genre.name, hint });
}

/** `3 albums · 41 tracks` from what is known. */
function counts(genre: Genre | undefined): string {
  if (!genre) return "";
  return [
    genre.albumCount !== null ? plural(genre.albumCount, "album") : null,
    genre.trackCount !== null ? plural(genre.trackCount, "track") : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

export function GenresView({ entry, source }: { entry: Entry; source: SourceKind }) {
  const version = useLibraryVersion();
  const load = useLoad(() => browse.genres(source), source, source === "local" ? version : null);
  const scroller = useRef<HTMLDivElement>(null);
  const genres = load.data ?? [];
  useScroll(scroller, entry, genres.length > 0);
  const { height } = useElementSize(scroller);
  const idPrefix = useId();
  // Kept by id, so a quiet refresh (the scan adding a genre above it)
  // keeps the same genre selected; cleared when it disappears.
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const found = selectedId !== null ? genres.findIndex((g) => g.id === selectedId) : -1;
  const selected = found >= 0 ? found : null;
  useShuffleContext(() => shuffleAll(source));

  const onKeyDown = (e: KeyboardEvent) => {
    if (genres.length === 0) return;
    const at = selected ?? -1;
    const row = document.getElementById(`${idPrefix}-0`)?.offsetHeight || 1;
    const page = Math.max(1, Math.floor(height / row) - 1);
    let to: number | null = null;
    switch (e.key) {
      case "ArrowDown":
        to = at + 1;
        break;
      case "ArrowUp":
        to = at - 1;
        break;
      case "PageDown":
        to = at + page;
        break;
      case "PageUp":
        to = at - page;
        break;
      case "Home":
        to = 0;
        break;
      case "End":
        to = genres.length - 1;
        break;
      case "Enter":
        if (selected !== null) openGenre(source, genres[selected], genres[selected]);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (to !== null) {
      const index = Math.min(genres.length - 1, Math.max(0, to));
      setSelectedId(genres[index].id);
      document.getElementById(`${idPrefix}-${index}`)?.scrollIntoView({ block: "nearest" });
    }
  };

  return (
    <section className="view">
      <ViewHeader
        eyebrow={SOURCE_LABEL[source]}
        title="Genres"
        actions={<PlayButtons onShuffle={() => shuffleAll(source)} />}
      >
        {load.state === "ready" && (
          <div className="secondary">{plural(genres.length, "genre")}</div>
        )}
      </ViewHeader>
      {source === "local" && <ScanLine />}
      <div className="view-body" ref={scroller}>
        {load.state === "loading" && <SkeletonRows />}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {load.state === "ready" && genres.length === 0 && (
          <Empty title="No genres">
            {source === "local"
              ? "No scanned track has a genre tag."
              : "The server lists no genres."}
          </Empty>
        )}
        {genres.length > 0 && (
          <div
            className="genre-list"
            role="listbox"
            aria-label="Genres"
            tabIndex={0}
            aria-activedescendant={selected !== null ? `${idPrefix}-${selected}` : undefined}
            onKeyDown={onKeyDown}
          >
            {genres.map((genre, index) => (
              <div
                key={genre.id}
                id={`${idPrefix}-${index}`}
                role="option"
                aria-selected={selected === index}
                className={`genre-row ${selected === index ? "is-selected" : ""}`}
                onClick={() => {
                  setSelectedId(genre.id);
                  openGenre(source, genre, genre);
                }}
              >
                <span className="genre-name" title={genre.name}>
                  {genre.name}
                </span>
                <span className="secondary">{counts(genre)}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

interface GenreProps {
  entry: Entry;
  source: SourceKind;
  id: string;
  name: string;
  hint?: Genre;
}

export function GenreView(props: GenreProps) {
  return props.source === "local" ? <LocalGenre {...props} /> : <NavidromeGenre {...props} />;
}

function LocalGenre({ entry, id, name, hint }: GenreProps) {
  const version = useLibraryVersion();
  const load = useLoad(() => browse.localGenre(id), id, version);
  const scroller = useRef<HTMLDivElement>(null);
  const albums = load.data?.albums ?? [];
  useScroll(scroller, entry, albums.length > 0);
  const genre = load.data?.genre ?? hint;
  const title = genre?.name ?? name;
  // The engine resolves the genre (no track list over IPC).
  const play = (shuffle: boolean) => playLocal({ kind: "genre", id }, shuffle);
  useShuffleContext(load.state === "ready" && albums.length > 0 ? () => play(true) : null);
  return (
    <section className="view">
      <ViewHeader
        eyebrow={`${SOURCE_LABEL.local} · Genre`}
        title={title}
        actions={
          <PlayButtons
            onPlay={() => play(false)}
            onShuffle={() => play(true)}
            disabled={albums.length === 0}
          />
        }
      >
        <div className="secondary">{counts(genre)}</div>
      </ViewHeader>
      <ScanLine />
      <div className="view-body" ref={scroller}>
        {load.state === "loading" && <SkeletonGrid count={6} />}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {load.state === "ready" && albums.length === 0 && <Empty title="No albums" />}
        {albums.length > 0 && (
          <AlbumGrid
            albums={albums}
            source="local"
            scroller={scroller}
            label={`${title} albums`}
          />
        )}
      </div>
    </section>
  );
}

function NavidromeGenre({ entry, id, name, hint }: GenreProps) {
  const scroller = useRef<HTMLDivElement>(null);
  const paged = usePagedAlbums((offset) => browse.navidromeGenreAlbums(id, offset), id);
  const { pages } = paged;
  useScroll(scroller, entry, pages.albums.length > 0);
  const title = hint?.name ?? name;
  // Random songs of the genre, as served (already random).
  const shuffle = () =>
    playFrom(async () => {
      const songs = await browse.genreRandomSongs(id);
      if (songs.length === 0) showUiNotice("No songs found in this genre");
      else playTracks(songs);
    });
  const empty = pages.albums.length === 0 && pages.next === null;
  useShuffleContext(empty ? null : shuffle);
  return (
    <section className="view">
      <ViewHeader
        eyebrow={`${SOURCE_LABEL.navidrome} · Genre`}
        title={title}
        actions={<PlayButtons onShuffle={shuffle} disabled={empty} />}
      >
        <div className="secondary">
          {hint
            ? counts(hint)
            : pages.albums.length > 0 &&
              `${plural(pages.albums.length, "album")}${pages.next !== null ? "+" : ""}`}
        </div>
      </ViewHeader>
      <div className="view-body" ref={scroller}>
        <PagedAlbumsBody
          paged={paged}
          scroller={scroller}
          label={`${title} albums`}
          empty="The server lists no albums in this genre."
        />
      </div>
    </section>
  );
}
