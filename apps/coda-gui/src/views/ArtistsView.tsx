// All artists of a source: an alphabetical, virtualized list with a
// letter index on the right. ↑/↓ select, Enter or click opens.

import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { useElementSize, useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { shuffleAll, useShuffleContext } from "../lib/play";
import type { Artist, SourceKind } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { PlayButtons, ScanLine, SOURCE_LABEL, ViewHeader, plural } from "./common";

const ROW = 48;
const OVERSCAN = 8;

function letterOf(name: string): string {
  const first = [...name.trim()][0]?.toUpperCase() ?? "#";
  return /\p{L}/u.test(first) ? first.normalize("NFD")[0] : "#";
}

export function openArtist(source: SourceKind, artist: Artist) {
  nav.push({ kind: "artist", source, id: artist.id, name: artist.name });
}

export function ArtistsView({ entry, source }: { entry: Entry; source: SourceKind }) {
  const version = useLibraryVersion();
  const load = useLoad(() => browse.artists(source), source, source === "local" ? version : null);
  const scroller = useRef<HTMLDivElement>(null);
  const artists = load.data ?? [];
  const scrollTop = useScroll(scroller, entry, artists.length > 0);
  const { height } = useElementSize(scroller);
  const [selected, setSelected] = useState<number | null>(null);
  useShuffleContext(() => shuffleAll(source));

  const letters = useMemo(() => {
    const first = new Map<string, number>();
    artists.forEach((a, i) => {
      const letter = letterOf(a.name);
      if (!first.has(letter)) first.set(letter, i);
    });
    return [...first.entries()];
  }, [artists]);

  const scrollTo = (index: number) => {
    const el = scroller.current;
    if (!el) return;
    const top = index * ROW;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW - el.clientHeight;
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (artists.length === 0) return;
    const at = selected ?? -1;
    const page = Math.max(1, Math.floor(height / ROW) - 1);
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
        to = artists.length - 1;
        break;
      case "Enter":
        if (selected !== null) openArtist(source, artists[selected]);
        break;
      default:
        // Letters stay global shortcuts; the index on the right jumps.
        return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (to !== null) {
      const index = Math.min(artists.length - 1, Math.max(0, to));
      setSelected(index);
      scrollTo(index);
    }
  };

  const first = Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN);
  const last = Math.min(artists.length, Math.ceil((scrollTop + height) / ROW) + OVERSCAN);

  return (
    <section className="view">
      <ViewHeader
        eyebrow={SOURCE_LABEL[source]}
        title="Artists"
        actions={<PlayButtons onShuffle={() => shuffleAll(source)} />}
      >
        {load.state === "ready" && (
          <div className="secondary">{plural(artists.length, "artist")}</div>
        )}
      </ViewHeader>
      {source === "local" && <ScanLine />}
      <div className="artists-layout">
        <div className="view-body list-body" ref={scroller}>
          {load.state === "loading" && <SkeletonRows />}
          {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
          {load.state === "ready" && artists.length === 0 && (
            <Empty title="No artists">
              {source === "local"
                ? "Nothing has been scanned yet."
                : "The server lists no artists."}
            </Empty>
          )}
          {artists.length > 0 && (
            <div
              className="artist-list"
              role="listbox"
              aria-label="Artists"
              tabIndex={0}
              aria-activedescendant={selected !== null ? `artist-${selected}` : undefined}
              style={{ height: artists.length * ROW }}
              onKeyDown={onKeyDown}
            >
              {artists.slice(first, last).map((artist, i) => {
                const index = first + i;
                const count = artist.albumCount !== null ? plural(artist.albumCount, "album") : "";
                return (
                  <div
                    key={artist.id}
                    id={`artist-${index}`}
                    role="option"
                    aria-selected={selected === index}
                    className={`artist-row ${selected === index ? "is-selected" : ""}`}
                    style={{ top: index * ROW }}
                    onClick={() => {
                      setSelected(index);
                      openArtist(source, artist);
                    }}
                  >
                    <Cover
                      url={artist.coverUrl}
                      name={artist.name}
                      size="small"
                      className="artist-thumb"
                    />
                    <span className="artist-name">{artist.name}</span>
                    <span className="secondary">{count}</span>
                  </div>
                );
              })}
            </div>
          )}
        </div>
        {letters.length > 1 && (
          <nav className="letter-index" aria-label="Jump to letter">
            {letters.map(([letter, index]) => (
              <button
                key={letter}
                tabIndex={-1}
                onClick={() => {
                  setSelected(index);
                  const el = scroller.current;
                  if (el) el.scrollTop = index * ROW;
                }}
              >
                {letter}
              </button>
            ))}
          </nav>
        )}
      </div>
    </section>
  );
}
