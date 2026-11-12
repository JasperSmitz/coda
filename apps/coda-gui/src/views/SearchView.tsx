// Search (`/` or Ctrl+F): a debounced query against the active source,
// results grouped into artists, albums and tracks, with a link to search
// the other source instead. Enter on a track plays the matching tracks
// from it. A Lyrics section follows, asked for once the main results are
// in (so their songs are left out of it): the local library's lyrics, or
// the Navidrome lyrics index (with a note while it is still being built;
// absent when the index is off).

import { useEffect, useMemo, useRef, useState } from "react";
import { AlbumGrid } from "../components/AlbumGrid";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { TrackTable } from "../components/TrackTable";
import { SearchIcon } from "../components/icons";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { playTracks, useShuffleContext } from "../lib/play";
import type { LyricsSearch, SearchResults, SourceKind, Track } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { bothConfigured, switchSource, useSources } from "../state/source";
import { openArtist } from "./ArtistsView";
import { SOURCE_LABEL, ViewHeader, plural } from "./common";

const DEBOUNCE_MS = 250;
/** Shorter queries search no lyrics (the backend's rule, checked early). */
const MIN_LYRICS_CHARS = 3;

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
  const scroller = useRef<HTMLDivElement>(null);

  useEffect(() => {
    input.current?.focus();
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
    <section className="view">
      <ViewHeader eyebrow={SOURCE_LABEL[source]} title="Search">
        <div className="search-box">
          <SearchIcon size={16} />
          <input
            ref={input}
            className="search-input"
            type="search"
            placeholder={
              source === "local"
                ? "Artists, albums, tracks or lyrics in the local library"
                : "Artists, albums, tracks or lyrics in Navidrome"
            }
            value={text}
            spellCheck={false}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") setSubmitted(text.trim());
              if (e.key === "ArrowDown") {
                e.preventDefault();
                scroller.current?.querySelector<HTMLElement>("[tabindex='0'], button")?.focus();
              }
            }}
            aria-label="Search"
          />
        </div>
        {bothConfigured(sources) && (
          <button
            className="link-button search-other"
            onClick={() => searchOther(other, text.trim())}
          >
            Search {other === "navidrome" ? "Navidrome" : "the local library"} instead
          </button>
        )}
      </ViewHeader>
      <div className="view-body" ref={scroller}>
        {submitted === "" ? (
          <Empty title="Search your music">
            Type to search {source === "local" ? "the local library" : "Navidrome"}.
          </Empty>
        ) : (
          <SourceResults key={source} source={source} query={submitted} scroller={scroller} />
        )}
      </div>
    </section>
  );
}

function SourceResults({
  source,
  query,
  scroller,
}: {
  source: SourceKind;
  query: string;
  scroller: React.RefObject<HTMLDivElement | null>;
}) {
  const mainKey = `${source}:${query}`;
  const load = useLoad<SearchResults>(() => browse.search(source, query), mainKey);
  const results = load.data;
  const tracks = results?.tracks ?? [];
  // `s` shuffles the matching tracks.
  useShuffleContext(tracks.length > 0 ? () => playTracks(tracks, undefined, true) : null);
  // The results are this query's (on the render where the query changes,
  // the previous query's are still returned).
  const mainDone = load.state !== "loading" && load.key === mainKey;

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
      {results && results.artists.length > 0 && (
        <>
          <h3 className="group-title">Artists</h3>
          <div className="chip-list" role="list">
            {results.artists.map((artist) => (
              <button
                key={artist.id}
                role="listitem"
                className="artist-chip"
                onClick={() => openArtist(source, artist)}
              >
                <Cover
                  url={artist.coverUrl}
                  name={artist.name}
                  size="small"
                  className="chip-cover"
                />
                <span>{artist.name}</span>
              </button>
            ))}
          </div>
        </>
      )}
      {results && results.albums.length > 0 && (
        <>
          <h3 className="group-title">Albums</h3>
          <AlbumGrid
            albums={results.albums}
            source={source}
            scroller={scroller}
            label={`${SOURCE_LABEL[source]} albums`}
          />
        </>
      )}
      {results && tracks.length > 0 && (
        <>
          <h3 className="group-title">
            Tracks
            {results.totalTracks > tracks.length && (
              <span className="secondary">
                {" "}
                · first {tracks.length.toLocaleString()} of {plural(results.totalTracks, "match")}
              </span>
            )}
          </h3>
          <TrackTable
            tracks={tracks}
            scroller={scroller}
            showArtist
            showAlbum
            label={`${SOURCE_LABEL[source]} tracks`}
          />
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
  scroller: React.RefObject<HTMLDivElement | null>;
}) {
  const tracks = useMemo<Track[]>(() => found.matches.map((m) => m.track), [found]);
  const snippets = useMemo(() => found.matches.map((m) => m.snippet), [found]);
  const more = found.total - found.matches.length;
  return (
    <>
      <h3 className="group-title">Lyrics</h3>
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
