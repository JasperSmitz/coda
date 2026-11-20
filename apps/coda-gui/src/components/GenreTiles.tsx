// Genres as Covers (DESIGN.md "Library"): tiles in a responsive grid,
// each coloured by its first album's cover (its swatches as light; a
// greyscale cover's tint; the house glass until known), the album count, the name in
// display type and that cover peeking, tilted, from the top corner.
// Local genres name their first album's cover; a Navidrome genre's is
// looked up (its first page of albums, cached like the genre page's, one
// genre at a time) while its tile is near the view. The arrow keys, Home
// and End move between tiles, Enter opens one, the context-menu key (or
// Shift+F10) opens its menu.

import { memo, useEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { useNear, useSeen } from "../hooks/useSeen";
import { browse } from "../lib/browse";
import { createLookupQueue } from "../lib/lookupQueue";
import type { Genre, SourceKind } from "../lib/types";
import { usePalette } from "../state/palettes";
import { gridKeyTarget } from "./AlbumGrid";
import { Cover } from "./Cover";

// ----- a Navidrome genre's cover -----

/**
 * Navidrome genres' covers: the first cover of the genre's first page of
 * albums (the genre page's own cached request), one lookup at a time.
 * A tile asks only while near the view and gives up its turn when it
 * leaves it or unmounts; leaving Genres or a server change drops the rest.
 */
const covers = createLookupQueue<string>((id) =>
  browse
    .navidromeGenreAlbums(id, 0)
    .then((page) => page.albums.find((a) => a.coverUrl !== null)?.coverUrl ?? null),
);

/** Forgets the looked-up covers and drops the queue (the server changed). */
export function forgetGenreCovers() {
  covers.clear();
}

/** The genre's cover: local, as listed; Navidrome, looked up while `near`. */
function useGenreCover(source: SourceKind, genre: Genre, near: boolean): string | null {
  const [found, setFound] = useState<string | null>(() =>
    source === "navidrome" ? (covers.known(genre.id) ?? null) : null,
  );
  useEffect(() => {
    if (source !== "navidrome" || !near) return;
    // The cleanup gives up the turn (left the view, unmounted).
    return covers.request(genre.id, setFound);
  }, [source, genre.id, near]);
  return source === "local" ? genre.coverUrl : found;
}

// ----- the tiles -----

export function GenreTiles({
  genres,
  source,
  label,
  onOpen,
  onMenu,
}: {
  genres: Genre[];
  source: SourceKind;
  label: string;
  onOpen: (genre: Genre) => void;
  onMenu: (genre: Genre, x: number, y: number) => void;
}) {
  const grid = useRef<HTMLDivElement>(null);
  // Leaving Genres: no lookup left waiting (the tiles' own cleanups give
  // up their turns too; this also drops a turn a tile never took back).
  useEffect(() => () => covers.drop(), []);

  /** Tiles per row, from where they are laid out. */
  const columns = () => {
    const tiles = grid.current?.querySelectorAll<HTMLElement>(".genre-tile");
    if (!tiles || tiles.length === 0) return 1;
    const top = tiles[0].offsetTop;
    let n = 0;
    while (n < tiles.length && tiles[n].offsetTop === top) n++;
    return Math.max(1, n);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const target = e.target as HTMLElement;
    const at = Number(target.dataset.index ?? -1);
    if (at < 0) return;
    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      const rect = target.getBoundingClientRect();
      onMenu(genres[at], rect.left + 24, rect.top + 40);
      e.preventDefault();
      return;
    }
    const to = gridKeyTarget(e.key, at, columns(), genres.length);
    if (to === null) return;
    // Not the global seek.
    e.preventDefault();
    e.stopPropagation();
    const el = grid.current?.querySelector<HTMLElement>(`[data-index="${to}"]`);
    el?.focus({ preventScroll: true });
    el?.scrollIntoView({ block: "nearest" });
  };

  return (
    <div ref={grid} className="genre-grid" role="list" aria-label={label} onKeyDown={onKeyDown}>
      {genres.map((genre, index) => (
        <GenreTile
          key={genre.id}
          genre={genre}
          index={index}
          source={source}
          onOpen={onOpen}
          onMenu={onMenu}
        />
      ))}
    </div>
  );
}

const GenreTile = memo(function GenreTile({
  genre,
  index,
  source,
  onOpen,
  onMenu,
}: {
  genre: Genre;
  index: number;
  source: SourceKind;
  onOpen: (genre: Genre) => void;
  onMenu: (genre: Genre, x: number, y: number) => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const near = useNear(ref);
  const seen = useSeen(ref);
  const cover = useGenreCover(source, genre, near);
  const palette = usePalette(seen ? cover : null);
  // Swatches (the cover's vivid colours) as light, else its tint (dark by
  // construction, core `cover::palette`): the text's contrast over them
  // is in browse.css `.genre-tile`.
  const swatches = palette?.swatches ?? [];
  const style = palette
    ? ({
        "--genre-tint": palette.tint,
        "--genre-swatch-1": swatches[0],
        "--genre-swatch-2": swatches[1] ?? swatches[0],
      } as CSSProperties)
    : undefined;
  const look = swatches.length > 0 ? "has-swatches" : palette ? "is-tinted" : "";
  const count = genre.albumCount;
  return (
    <div role="listitem" className="genre-tile-item">
      <button
        ref={ref}
        className={`genre-tile ${look}`}
        data-index={index}
        style={style}
        title={genre.name}
        onClick={() => onOpen(genre)}
        onContextMenu={(e) => {
          e.preventDefault();
          onMenu(genre, e.clientX, e.clientY);
        }}
      >
        {cover && (
          <span className="genre-tile-peek" aria-hidden="true">
            <Cover url={cover} name={genre.name} size="small" className="genre-tile-cover" />
          </span>
        )}
        <span className="genre-tile-text">
          {count !== null && (
            <span className="genre-tile-count">
              {count.toLocaleString()} {count === 1 ? "album" : "albums"}
            </span>
          )}
          <span className="genre-tile-name">{genre.name}</span>
        </span>
      </button>
    </div>
  );
});
