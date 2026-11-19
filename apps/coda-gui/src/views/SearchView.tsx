// Search (`/` or Ctrl+F): a large field at the top of the page, a
// debounced query against the active source, with a link to search the
// other source instead. Empty: the source's genres as chips. Results: the
// Top result (the best album or artist match) beside the first Songs,
// then Albums and Artists. Enter on a song plays the matching songs from
// it. A Lyrics section follows, asked for once the main results are in
// (so their songs are left out of it): the local library's lyrics, or
// the Navidrome lyrics index (with a note while it is still being built;
// absent when the index is off).

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AlbumGrid, openAlbum } from "../components/AlbumGrid";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { TrackTable } from "../components/TrackTable";
import { SearchIcon } from "../components/icons";
import { usePageScroller, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { morphKey } from "../lib/pageTransition";
import { browse } from "../lib/browse";
import { playTracks, useShuffleContext } from "../lib/play";
import { genreChips, topResult, type TopResult } from "../lib/search";
import type { LyricsSearch, SearchResults, SourceKind, Track } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { bothConfigured, switchSource, useSources } from "../state/source";
import { openArtist } from "./ArtistsView";
import { SOURCE_LABEL, plural } from "./common";
import { openGenre } from "./GenresView";

const DEBOUNCE_MS = 250;
/** Shorter queries search no lyrics (the backend's rule, checked early). */
const MIN_LYRICS_CHARS = 3;
/** Songs shown beside the Top result before "Show all". */
const SONGS_SHOWN = 5;
/** Genre chips on the empty page (the largest genres). */
const GENRE_CHIPS = 30;

interface Props {
  entry: Entry;
  source: SourceKind;
  query: string;
}

/** Searches the other source for `query` (on its own stack). */
function searchOther(source: SourceKind, query: string) {
  switchSource(source);
  if (nav.source() === source) nav.search(query);
}

export function SearchView({ entry, source, query }: Props) {
  const sources = useSources();
  const other: SourceKind = source === "local" ? "navidrome" : "local";
  const [text, setText] = useState(query);
  const [submitted, setSubmitted] = useState(query.trim());
  const input = useRef<HTMLInputElement>(null);
  const results = useRef<HTMLDivElement>(null);
  const scroller = usePageScroller();
  // Back to a search: its place comes back once the results (or the
  // genres) are in, not while they load.
  const [settled, setSettled] = useState(false);
  const onSettled = useCallback(() => setSettled(true), []);
  useScroll(scroller, entry, settled);

  // Without scrolling: Back keeps the page's place (`/` and Ctrl+F focus
  // the field with a scroll, `nav.search`).
  useEffect(() => {
    input.current?.focus({ preventScroll: true });
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      setSubmitted(text.trim());
      // Remember the query on this entry, so Back returns to it.
      if (entry.route.kind === "search") entry.route = { kind: "search", source, query: text };
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [text, entry, source]);

  return (
    <section className="view search-view">
      <h1 className="visually-hidden">Search</h1>
      <div className="search-head">
        {/* A label: a click on its icon or padding focuses the field. */}
        <label className="search-box">
          <SearchIcon size={20} />
          <input
            ref={input}
            className="search-input"
            type="search"
            placeholder="Artists, albums, songs or lyrics"
            value={text}
            spellCheck={false}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") setSubmitted(text.trim());
              if (e.key === "ArrowDown") {
                e.preventDefault();
                results.current?.querySelector<HTMLElement>("[tabindex='0'], button")?.focus();
              }
            }}
            aria-label={`Search ${source === "local" ? "the local library" : "Navidrome"}`}
          />
        </label>
        {bothConfigured(sources) && (
          <button
            className="link-button search-other"
            onClick={() => searchOther(other, text.trim())}
          >
            Search {other === "navidrome" ? "Navidrome" : "the local library"} instead
          </button>
        )}
      </div>
      <div className="view-body search-results" ref={results}>
        {submitted === "" ? (
          <GenreChips key={source} source={source} onSettled={onSettled} />
        ) : (
          <SourceResults key={source} source={source} query={submitted} onSettled={onSettled} />
        )}
      </div>
    </section>
  );
}

/** The empty page: the source's largest genres, each opening its page. */
function GenreChips({ source, onSettled }: { source: SourceKind; onSettled: () => void }) {
  const load = useLoad(() => browse.genres(source), `genres:${source}`);
  const done = load.state !== "loading";
  useEffect(() => {
    if (done) onSettled();
  }, [done, onSettled]);
  const chips = useMemo(() => genreChips(load.data ?? [], GENRE_CHIPS), [load.data]);
  const all = load.data?.length ?? 0;
  if (load.state === "loading") return null;
  if (chips.length === 0) {
    return (
      <Empty title="Search your music">
        Type to search {source === "local" ? "the local library" : "Navidrome"}.
      </Empty>
    );
  }
  return (
    <section className="search-section">
      <h2 className="group-title">Genres</h2>
      <ul className="genre-chips" aria-label="Genres">
        {chips.map((genre) => (
          <li key={genre.id}>
            <button
              className="genre-chip"
              title={genre.name}
              onClick={() => openGenre(source, genre, genre)}
            >
              {genre.name}
            </button>
          </li>
        ))}
        {all > chips.length && (
          <li>
            <button
              className="genre-chip genre-chip-all"
              onClick={() => nav.push({ kind: "genres", source })}
            >
              All {plural(all, "genre")}
            </button>
          </li>
        )}
      </ul>
    </section>
  );
}

function SourceResults({
  source,
  query,
  onSettled,
}: {
  source: SourceKind;
  query: string;
  /** This query's results are in (Back restores the page's place then). */
  onSettled: () => void;
}) {
  const scroller = usePageScroller();
  const mainKey = `${source}:${query}`;
  const load = useLoad<SearchResults>(() => browse.search(source, query), mainKey);
  const results = load.data;
  const tracks = results?.tracks ?? [];
  // "Show all" holds for the query it was asked for.
  const [allFor, setAllFor] = useState<string | null>(null);
  const allSongs = allFor === mainKey;
  // `s` shuffles the matching tracks.
  useShuffleContext(tracks.length > 0 ? () => playTracks(tracks, undefined, true) : null);
  // The results are this query's (on the render where the query changes,
  // the previous query's are still returned).
  const mainDone = load.state !== "loading" && load.key === mainKey;
  useEffect(() => {
    if (mainDone) onSettled();
  }, [mainDone, onSettled]);

  // Lyrics: once this query's results are in, without their songs (the
  // local backend also leaves out every track local search matches). One
  // lyrics search per query.
  const lyricsQuery =
    mainDone && results && [...query].length >= MIN_LYRICS_CHARS ? query : null;
  const exclude = lyricsQuery === null ? [] : songIds(tracks);
  const lyricsKey = `lyrics:${source}:${lyricsQuery ?? ""}:${exclude.join(",")}`;
  const lyrics = useLoad<LyricsSearch | null>(
    () =>
      lyricsQuery === null ? Promise.resolve(null) : browse.lyrics(source, lyricsQuery, exclude),
    lyricsKey,
  );
  const lyricsDone = lyrics.state !== "loading" && lyrics.key === lyricsKey;
  const lyricsFound = lyricsDone ? lyrics.data : null;
  const lyricMatches = lyricsFound?.matches ?? [];
  const lyricsPending = lyricsQuery !== null && !lyricsDone;

  const top = useMemo(
    () => (results ? topResult(query, results.artists, results.albums) : null),
    [results, query],
  );
  const songs = allSongs ? tracks : tracks.slice(0, SONGS_SHOWN);

  const empty =
    mainDone &&
    results &&
    results.artists.length === 0 &&
    results.albums.length === 0 &&
    results.tracks.length === 0 &&
    lyricMatches.length === 0 &&
    !lyricsPending;
  return (
    <section className="search-section">
      {load.state === "loading" && <SkeletonRows count={5} />}
      {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
      {empty && <p className="secondary search-none">No matches for “{query}”.</p>}
      {results && (top || tracks.length > 0) && (
        <div className={`search-top ${top && tracks.length > 0 ? "has-both" : ""}`}>
          {top && (
            <div className="search-top-result">
              <h2 className="group-title">Top result</h2>
              <TopResultCard source={source} top={top} />
            </div>
          )}
          {tracks.length > 0 && (
            <div className="search-songs">
              <h2 className="group-title">
                Songs
                {results.totalTracks > tracks.length && (
                  <span className="secondary">
                    {" "}
                    · first {tracks.length.toLocaleString()} of{" "}
                    {plural(results.totalTracks, "match", "matches")}
                  </span>
                )}
              </h2>
              <TrackTable
                tracks={songs}
                scroller={scroller}
                covers
                showArtist
                onPlay={(index) => playTracks(tracks, index)}
                label={`${SOURCE_LABEL[source]} songs`}
              />
              {tracks.length > SONGS_SHOWN && (
                <button className="text-button search-more" onClick={() => setAllFor(allSongs ? null : mainKey)}>
                  {allSongs ? "Show fewer" : `Show all ${plural(tracks.length, "song")}`}
                </button>
              )}
            </div>
          )}
        </div>
      )}
      {results && results.albums.length > 0 && (
        <>
          <h2 className="group-title">Albums</h2>
          <AlbumGrid
            albums={results.albums}
            source={source}
            scroller={scroller}
            label={`${SOURCE_LABEL[source]} albums`}
          />
        </>
      )}
      {results && results.artists.length > 0 && (
        <>
          <h2 className="group-title">Artists</h2>
          <div className="artist-tiles" role="list" aria-label={`${SOURCE_LABEL[source]} artists`}>
            {results.artists.map((artist) => (
              <button
                key={artist.id}
                role="listitem"
                className="artist-tile"
                title={artist.name}
                onClick={() => openArtist(source, artist)}
              >
                <Cover
                  url={artist.coverUrl}
                  name={artist.name}
                  size="small"
                  className="artist-tile-cover"
                  morph={morphKey("artist", source, artist.id)}
                />
                <span className="artist-tile-name">{artist.name}</span>
                <span className="secondary">Artist</span>
              </button>
            ))}
          </div>
        </>
      )}
      {lyricsFound && lyricMatches.length > 0 && (
        <LyricsSection found={lyricsFound} scroller={scroller} />
      )}
      {lyricsFound?.indexing && (
        <p className="secondary search-note">
          {lyricsFound.indexing.total === 0
            ? "The lyrics index is still loading."
            : `Lyrics are still being indexed (${lyricsFound.indexing.done.toLocaleString()} of ${plural(lyricsFound.indexing.total, "song")}).`}
        </p>
      )}
      {results && !empty && source === "navidrome" && (
        <p className="secondary search-note">
          The server returns at most 20 artists, 20 albums and 50 tracks.{" "}
          <button className="link-button" onClick={() => nav.push({ kind: "artists", source })}>
            Browse all artists
          </button>
        </p>
      )}
    </section>
  );
}

/** The Top result: a card with a large cover, the name in display type, and what it is. */
function TopResultCard({ source, top }: { source: SourceKind; top: TopResult }) {
  const artist = top.kind === "artist";
  const name = artist ? top.artist.name : top.album.name;
  const coverUrl = artist ? top.artist.coverUrl : top.album.coverUrl;
  const kind = artist ? "Artist" : `Album · ${top.album.artist ?? "Unknown artist"}`;
  return (
    <button
      className="top-result"
      title={name}
      onClick={() => (artist ? openArtist(source, top.artist) : openAlbum(source, top.album))}
    >
      <Cover
        url={coverUrl}
        name={name}
        size="large"
        className={`top-result-cover ${artist ? "round" : ""}`}
        morph={
          artist
            ? morphKey("artist", source, top.artist.id)
            : morphKey("album", source, top.album.id)
        }
      />
      <span className="top-result-name">{name}</span>
      <span className="top-result-kind secondary">{kind}</span>
    </button>
  );
}

/** The ids of `tracks`: local ids (those in the library) or Navidrome song ids. */
function songIds(tracks: Track[]): string[] {
  return tracks.flatMap((t) => {
    if (t.source.kind === "local") return t.source.id ? [t.source.id] : [];
    return [t.source.songId];
  });
}

/** Lyric matches: rows like the songs above, with the lyric line below. */
function LyricsSection({
  found,
  scroller,
}: {
  found: LyricsSearch;
  scroller: React.RefObject<HTMLElement | null>;
}) {
  const tracks = useMemo<Track[]>(() => found.matches.map((m) => m.track), [found]);
  const snippets = useMemo(() => found.matches.map((m) => m.snippet), [found]);
  const more = found.total - found.matches.length;
  return (
    <>
      <h2 className="group-title">Lyrics</h2>
      <TrackTable
        tracks={tracks}
        snippets={snippets}
        scroller={scroller}
        showArtist
        showAlbum
        label="Lyrics matches"
      />
      {more > 0 && <p className="secondary search-note">and {more.toLocaleString()} more</p>}
    </>
  );
}
