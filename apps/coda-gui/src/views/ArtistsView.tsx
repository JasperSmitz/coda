// All artists of a source: an alphabetical, virtualized list with a
// letter index on the right. ↑/↓ select, Enter or click opens; a row's
// menu (right-click or the context-menu key) offers "Add to mix…".

import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ContextMenu, type MenuState } from "../components/ContextMenu";
import { Cover } from "../components/Cover";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import {
  clearHeight,
  revealRow,
  scrollToTop,
  useElementSize,
  useLibraryVersion,
  useOffsetTop,
  usePageScroller,
  useScroll,
} from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { addToMixFlow } from "../lib/mixes";
import { artistIngredient } from "../lib/mixRecipe";
import { shuffleAll, useShuffleContext } from "../lib/play";
import type { Artist, SourceKind } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { useAppState } from "../state/store";
import { ListHeader, PlayButtons, ScanLine, SOURCE_LABEL, plural } from "./common";

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
  const scroller = usePageScroller();
  const artists = load.data ?? [];
  const scrollTop = useScroll(scroller, entry, artists.length > 0);
  const { height } = useElementSize(scroller);
  const [selected, setSelected] = useState<number | null>(null);
  useShuffleContext(() => shuffleAll(source));
  const [menu, setMenu] = useState<(MenuState & { artist: Artist }) | null>(null);
  const list = useRef<HTMLDivElement>(null);
  // Where the list starts in the page (below the header and the tools).
  const offset = useOffsetTop(list);
  const { snapshot } = useAppState();
  const started = snapshot?.phase === "ready";

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
    revealRow(el, offset + index * ROW, ROW);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (artists.length === 0) return;
    const at = selected ?? -1;
    const room = scroller.current ? clearHeight(scroller.current) : height;
    const page = Math.max(1, Math.floor(room / ROW) - 1);
    let to: number | null = null;
    // Shift+F10 opens the row's menu like the context-menu key.
    const key = e.shiftKey && e.key === "F10" ? "ContextMenu" : e.key;
    switch (key) {
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
      case "ContextMenu":
        if (selected !== null) {
          const row = document.getElementById(`artist-${selected}`)?.getBoundingClientRect();
          const artist = artists[selected];
          setMenu({ x: (row?.left ?? 0) + 40, y: row?.bottom ?? 0, key: artist.id, artist });
        }
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

  const first = Math.max(0, Math.floor((scrollTop - offset) / ROW) - OVERSCAN);
  const last = Math.min(artists.length, Math.ceil((scrollTop - offset + height) / ROW) + OVERSCAN);

  return (
    <section className="view">
      <ListHeader
        eyebrow={SOURCE_LABEL[source]}
        title="Artists"
        meta={load.state === "ready" && plural(artists.length, "artist")}
        tools={<PlayButtons onShuffle={() => shuffleAll(source)} />}
      />
      {source === "local" && <ScanLine />}
      <div className="artists-layout">
        <div className="view-body list-body">
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
              ref={list}
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
                    onContextMenu={(e) => {
                      e.preventDefault();
                      setSelected(index);
                      setMenu({ x: e.clientX, y: e.clientY, key: artist.id, artist });
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
                  if (el) scrollToTop(el, offset + index * ROW);
                }}
              >
                {letter}
              </button>
            ))}
          </nav>
        )}
      </div>
      {menu && (
        <ContextMenu
          state={menu}
          onClose={() => {
            setMenu(null);
            list.current?.focus();
          }}
          items={[
            { label: "Open", action: () => openArtist(source, menu.artist) },
            {
              label: "Add to mix…",
              disabled: !started,
              action: () => void addToMixFlow(source, artistIngredient(source, menu.artist)),
            },
          ]}
        />
      )}
    </section>
  );
}
