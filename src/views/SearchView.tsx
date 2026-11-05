// Search (`/` or Ctrl+F): a debounced query against each configured
// source, results grouped by source into artists, albums and tracks.
// Enter on a track plays the source's matching tracks from it.

import { useEffect, useRef, useState } from "react";
import { AlbumGrid } from "../components/AlbumGrid";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { TrackTable } from "../components/TrackTable";
import { SearchIcon } from "../components/icons";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { playTracks, useShuffleContext } from "../lib/play";
import type { SearchResults, SourceKind } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { useAppState } from "../state/store";
import { openArtist } from "./ArtistsView";
import { SOURCE_LABEL, ViewHeader, plural } from "./common";

const DEBOUNCE_MS = 250;

export function SearchView({ entry, query }: { entry: Entry; query: string }) {
  const { snapshot } = useAppState();
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
      if (entry.route.kind === "search") entry.route = { kind: "search", query: text };
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [text, entry]);

  const sources: SourceKind[] = [];
  if (snapshot && snapshot.scan.state !== "noLibrary") sources.push("local");
  if (snapshot && snapshot.navidrome.state !== "notConfigured") sources.push("navidrome");

  return (
    <section className="view">
      <ViewHeader title="Search">
        <div className="search-box">
          <SearchIcon size={16} />
          <input
            ref={input}
            className="search-input"
            type="search"
            placeholder="Artists, albums, tracks"
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
      </ViewHeader>
      <div className="view-body" ref={scroller}>
        {submitted === "" ? (
          <Empty title="Search your music">
            Type to search{" "}
            {sources.map((s) => SOURCE_LABEL[s].toLowerCase()).join(" and ") || "your library"}.
          </Empty>
        ) : sources.length === 0 ? (
          <Empty title="Nothing to search">No music folder or server is configured.</Empty>
        ) : (
          sources.map((source) => (
            <SourceResults
              key={source}
              source={source}
              query={submitted}
              scroller={scroller}
              only={sources.length === 1}
            />
          ))
        )}
      </div>
    </section>
  );
}

function SourceResults({
  source,
  query,
  scroller,
  only,
}: {
  source: SourceKind;
  query: string;
  scroller: React.RefObject<HTMLDivElement | null>;
  only: boolean;
}) {
  const load = useLoad<SearchResults>(() => browse.search(source, query), `${source}:${query}`);
  const results = load.data;
  const tracks = results?.tracks ?? [];
  // `s` shuffles the first source's tracks.
  useShuffleContext(
    tracks.length > 0 && (only || source === "local")
      ? () => playTracks(tracks, undefined, true)
      : null,
  );

  const empty =
    results &&
    results.artists.length === 0 &&
    results.albums.length === 0 &&
    results.tracks.length === 0;
  return (
    <section className="search-section">
      {!only && <h2 className="section-title">{SOURCE_LABEL[source]}</h2>}
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
