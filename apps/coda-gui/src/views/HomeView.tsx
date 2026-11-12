// Home (each source's landing view): rows of cards from the play history
// (Recently played, Most played), the server's Recently added, and the
// day's Daily Picks with "Shuffle Daily Picks". Rows scroll sideways with
// arrow buttons; ←/→ move between a row's cards, Tab between rows, Enter
// opens a card, Shift+Enter (or its hover button) plays it.

import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Cover } from "../components/Cover";
import { NameLink } from "../components/NameLink";
import { Empty, ErrorState, SkeletonGrid } from "../components/LoadState";
import { BackIcon, ForwardIcon, PlayIcon, ShuffleIcon } from "../components/icons";
import { useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import {
  albumContext,
  artistContext,
  openRoute,
  playFrom,
  playlistContext,
  playLocal,
  playTracks,
  useShuffleContext,
} from "../lib/play";
import type { Album, ContextKind, HomeItem, SourceKind } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { useAppState } from "../state/store";
import { SOURCE_LABEL, ViewHeader, useCanPlay } from "./common";

/** Cards per server row (the first page holds more). */
const ROW_MAX = 20;

/** The local calendar day, `YYYY-MM-DD`. */
export function localDate(now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/** Today, changing at local midnight while shown. */
function useToday(): string {
  const [today, setToday] = useState(localDate);
  useEffect(() => {
    const now = new Date();
    const midnight = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);
    const timer = window.setTimeout(
      () => setToday(localDate()),
      midnight.getTime() - now.getTime() + 1000,
    );
    return () => window.clearTimeout(timer);
  }, [today]);
  return today;
}

/** One card: a history item or an album from a list. */
interface Card {
  key: string;
  source: SourceKind;
  kind: ContextKind;
  id: string;
  path: string[] | null;
  name: string;
  subtitle: string | null;
  /** Albums: the artist (the subtitle), when known. */
  artistId: string | null;
  coverUrl: string | null;
  /** An album card's listing data (the album view's hint). */
  album?: Album;
}

function fromItem(item: HomeItem): Card {
  return { ...item, key: `${item.kind}:${item.id}` };
}

function fromAlbum(source: SourceKind, album: Album): Card {
  return {
    key: `album:${album.id}`,
    source,
    kind: "album",
    id: album.id,
    path: null,
    name: album.name,
    subtitle: album.artist,
    artistId: album.artistId,
    coverUrl: album.coverUrl,
    album,
  };
}

function itemAlbum(card: Card): Album {
  return (
    card.album ?? {
      id: card.id,
      name: card.name,
      artist: card.subtitle,
      artistId: card.artistId,
      year: null,
      trackCount: null,
      durationMs: null,
      coverUrl: card.coverUrl,
    }
  );
}

function openCard(card: Card) {
  const { source, id, name } = card;
  switch (card.kind) {
    case "album":
      nav.push({ kind: "album", source, id, name, hint: itemAlbum(card) });
      break;
    case "artist":
      nav.push({ kind: "artist", source, id, name });
      break;
    case "playlist":
      nav.push({ kind: "playlist", source, id, name });
      break;
    case "folder":
      nav.push({ kind: "folder", path: card.path ?? [] });
      break;
  }
}

function playCard(card: Card) {
  const { source, id, name, coverUrl } = card;
  switch (card.kind) {
    case "album":
      playFrom(async () =>
        playTracks(
          (await browse.album(source, id)).tracks,
          undefined,
          false,
          albumContext(source, itemAlbum(card)),
        ),
      );
      break;
    case "artist":
      if (source === "local") playLocal({ kind: "artist", id }, false);
      else
        playFrom(async () =>
          playTracks(
            await browse.navidromeArtistTracks(id),
            undefined,
            false,
            artistContext(source, { id, name, coverUrl }),
          ),
        );
      break;
    case "playlist":
      playFrom(async () =>
        playTracks(
          (await browse.playlist(source, id)).tracks,
          undefined,
          false,
          playlistContext(source, { id, name, coverUrl }),
        ),
      );
      break;
    case "folder":
      // The engine records the folder itself.
      playLocal({ kind: "folder", path: card.path ?? [] }, false);
      break;
  }
}

/** Every pick's tracks, shuffled together (no context of its own). */
function shufflePicks(source: SourceKind, picks: Album[]) {
  playFrom(async () => {
    const albums = await Promise.all(picks.map((album) => browse.album(source, album.id)));
    playTracks(
      albums.flatMap((a) => a.tracks),
      undefined,
      true,
    );
  });
}

const KIND_LABEL: Record<ContextKind, string> = {
  album: "Album",
  artist: "Artist",
  playlist: "Playlist",
  folder: "Folder",
};

function CardTile({
  card,
  tabbable,
  index,
  canPlay,
  showKind,
}: {
  card: Card;
  tabbable: boolean;
  index: number;
  canPlay: boolean;
  showKind: boolean;
}) {
  const sub = [showKind ? KIND_LABEL[card.kind] : null, card.subtitle].filter(Boolean).join(" · ");
  // An album's known artist is a link below the button (no button in a button).
  const artistId = card.kind === "album" ? card.artistId : null;
  const artist = card.subtitle;
  return (
    <li className="home-card">
      <button
        className="home-card-open"
        data-index={index}
        tabIndex={tabbable ? 0 : -1}
        title={card.name}
        onClick={() => openCard(card)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && e.shiftKey) {
            e.preventDefault();
            if (canPlay) playCard(card);
          }
        }}
      >
        <Cover
          url={card.coverUrl}
          name={card.name}
          size="small"
          className={`home-card-cover ${card.kind === "artist" ? "round" : ""}`}
        />
        <span className="album-title">{card.name}</span>
        {(artistId === null || !artist) && (
          <span className="album-sub secondary">{sub || " "}</span>
        )}
      </button>
      {artistId !== null && artist && (
        <div className="album-sub card-sub-link secondary">
          {showKind && `${KIND_LABEL[card.kind]} · `}
          <NameLink
            inRow
            label={`Go to artist ${artist}`}
            onGo={() =>
              openRoute({ kind: "artist", source: card.source, id: artistId, name: artist })
            }
          >
            {artist}
          </NameLink>
        </div>
      )}
      <button
        className="icon-button home-card-play"
        tabIndex={-1}
        disabled={!canPlay}
        aria-label={`Play ${card.name}`}
        title="Play (Shift+Enter)"
        onClick={() => playCard(card)}
      >
        <PlayIcon size={16} />
      </button>
    </li>
  );
}

function CardRow({
  title,
  cards,
  showKind = false,
  actions,
}: {
  title: string;
  cards: Card[];
  showKind?: boolean;
  actions?: ReactNode;
}) {
  const canPlay = useCanPlay();
  const list = useRef<HTMLUListElement>(null);
  const [focus, setFocus] = useState(0);
  const [edges, setEdges] = useState({ start: true, end: true });
  const at = Math.min(focus, Math.max(0, cards.length - 1));

  useEffect(() => {
    const el = list.current;
    if (!el) return;
    const update = () =>
      setEdges({
        start: el.scrollLeft <= 1,
        end: el.scrollLeft + el.clientWidth >= el.scrollWidth - 1,
      });
    update();
    el.addEventListener("scroll", update, { passive: true });
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => {
      el.removeEventListener("scroll", update);
      observer.disconnect();
    };
  }, [cards.length]);

  const page = (direction: 1 | -1) => {
    const el = list.current;
    if (!el) return;
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    el.scrollBy({ left: direction * el.clientWidth * 0.9, behavior: reduced ? "auto" : "smooth" });
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const target = e.target as HTMLElement;
    const index = Number(target.dataset.index ?? -1);
    if (index < 0) return;
    let to: number;
    switch (e.key) {
      case "ArrowRight":
        to = index + 1;
        break;
      case "ArrowLeft":
        to = index - 1;
        break;
      case "Home":
        to = 0;
        break;
      case "End":
        to = cards.length - 1;
        break;
      default:
        return;
    }
    // Not the global seek keys.
    e.preventDefault();
    e.stopPropagation();
    to = Math.min(cards.length - 1, Math.max(0, to));
    setFocus(to);
    const el = list.current?.querySelector<HTMLElement>(`[data-index="${to}"]`);
    el?.focus();
    el?.scrollIntoView({ block: "nearest", inline: "nearest" });
  };

  if (cards.length === 0) return null;
  return (
    <section className="home-row" aria-label={title}>
      <div className="home-row-header">
        <h2 className="section-title">{title}</h2>
        {actions}
        <div className="home-row-arrows">
          <button
            className="icon-button"
            tabIndex={-1}
            aria-label={`Scroll ${title} back`}
            disabled={edges.start}
            onClick={() => page(-1)}
          >
            <BackIcon size={16} />
          </button>
          <button
            className="icon-button"
            tabIndex={-1}
            aria-label={`Scroll ${title} on`}
            disabled={edges.end}
            onClick={() => page(1)}
          >
            <ForwardIcon size={16} />
          </button>
        </div>
      </div>
      <ul
        ref={list}
        className="home-cards"
        onKeyDown={onKeyDown}
        onFocus={(e) => {
          const index = Number((e.target as HTMLElement).dataset.index ?? -1);
          if (index >= 0) setFocus(index);
        }}
      >
        {cards.map((card, i) => (
          <CardTile
            key={card.key}
            card={card}
            index={i}
            tabbable={i === at}
            canPlay={canPlay}
            showKind={showKind}
          />
        ))}
      </ul>
    </section>
  );
}

const none = <T,>(): Promise<T[]> => Promise.resolve([]);

export function HomeView({ entry, source }: { entry: Entry; source: SourceKind }) {
  const version = useLibraryVersion();
  const { snapshot } = useAppState();
  const historyRevision = snapshot?.historyRevision ?? 0;
  const today = useToday();
  const canPlay = useCanPlay();
  const scroller = useRef<HTMLDivElement>(null);
  // Local rows resolve against the library (reload as it changes); both
  // sources reload when the history changes (a play from a card here).
  const history = useLoad(
    () => browse.homeHistory(source),
    `history:${source}`,
    source === "local" ? `${version}|${historyRevision}` : historyRevision,
  );
  // Local picks wait for the scan: drawn over a growing album set they
  // would change every refresh. They reload only once a scan has finished
  // (new library generation, so the shown ids stay valid).
  const waitForScan = source === "local" && snapshot?.scan.state === "scanning";
  const settled = `${version}|${snapshot?.browse.library ?? 0}`;
  const [picksVersion, setPicksVersion] = useState(settled);
  useEffect(() => {
    if (!waitForScan) setPicksVersion(settled);
  }, [waitForScan, settled]);
  const picks = useLoad(
    () => (waitForScan ? Promise.resolve(null) : browse.dailyPicks(source, today)),
    `picks:${source}:${today}`,
    source === "local" ? picksVersion : null,
  );
  // Navidrome only: the server's lists (built from scrobbles, the TUI's too).
  const frequent = useLoad(
    () =>
      source === "navidrome"
        ? browse.navidromeAlbums("frequent", 0).then((p) => p.albums.slice(0, ROW_MAX))
        : none<Album>(),
    `frequent:${source}`,
  );
  const newest = useLoad(
    () =>
      source === "navidrome"
        ? browse.navidromeAlbums("newest", 0).then((p) => p.albums.slice(0, ROW_MAX))
        : none<Album>(),
    `newest:${source}`,
  );
  const ready = history.state !== "loading" && picks.state !== "loading";
  useScroll(scroller, entry, ready);

  const pickAlbums = picks.data ?? [];
  useShuffleContext(pickAlbums.length > 0 ? () => shufflePicks(source, pickAlbums) : null);

  const recent = (history.data?.recent ?? []).map(fromItem);
  const most = [
    ...(frequent.data ?? []).map((a) => fromAlbum(source, a)),
    ...(history.data?.mostPlayed ?? []).map(fromItem),
  ];
  const added = (newest.data ?? []).map((a) => fromAlbum(source, a));
  const daily = pickAlbums.map((a) => fromAlbum(source, a));
  const fresh = history.state === "ready" && recent.length === 0 && most.length === 0;
  const nothing =
    ready &&
    !waitForScan &&
    frequent.state !== "loading" &&
    newest.state !== "loading" &&
    recent.length === 0 &&
    most.length === 0 &&
    added.length === 0 &&
    daily.length === 0;

  return (
    <section className="view">
      <ViewHeader eyebrow={SOURCE_LABEL[source]} title="Home" />
      <div className="view-body home-body" ref={scroller}>
        {!ready && <SkeletonGrid count={6} />}
        {history.state === "error" && <ErrorState error={history.error} onRetry={history.retry} />}
        {fresh && (
          <p className="secondary home-hint">
            Albums, artists, playlists{source === "local" ? " and folders" : ""} you play show up
            here.
          </p>
        )}
        <CardRow title="Recently played" cards={recent} showKind />
        <CardRow
          title="Most played"
          cards={most}
          showKind={source === "navidrome" || most.some((c) => c.kind !== "album")}
        />
        <CardRow title="Recently added" cards={added} />
        {waitForScan && daily.length === 0 && (
          <p className="secondary home-hint">Daily Picks are drawn once the library scan is done.</p>
        )}
        <CardRow
          title="Daily Picks"
          cards={daily}
          actions={
            <button
              className="button home-shuffle"
              disabled={!canPlay || pickAlbums.length === 0}
              onClick={() => shufflePicks(source, pickAlbums)}
              title="Shuffle Daily Picks (s)"
            >
              <ShuffleIcon size={14} /> Shuffle Daily Picks
            </button>
          }
        />
        {picks.state === "error" && history.state !== "error" && (
          <ErrorState error={picks.error} onRetry={picks.retry} />
        )}
        {nothing && history.state === "ready" && (
          <Empty title="Nothing here yet">
            {source === "local"
              ? "The library has no albums yet."
              : "The server has no albums yet."}
          </Empty>
        )}
      </div>
    </section>
  );
}
