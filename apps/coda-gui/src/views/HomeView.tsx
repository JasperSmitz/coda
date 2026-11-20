// Home (DESIGN.md "Home", 2.0 N6): Where you left off, a carousel of the
// recent listening contexts with Resume, and Moods for today, three quick
// mixes of three albums each. The page's glow and accent follow the
// carousel's focused cover (`usePageSubject`); the glow has faded out
// before the moods, which rest on the base.
//
// The layout is drawn at once: the carousel and the cards keep their
// room while their data loads. A listener without history gets a welcome
// line instead of the carousel. ←/→ move the carousel only while it has
// the focus (HomeCarousel); anywhere else on Home they seek, as on every
// page.

import { useCallback, useEffect, useState } from "react";
import { HomeCarousel, itemKey } from "../components/HomeCarousel";
import { MoodCard } from "../components/MoodCard";
import { ShuffleIcon } from "../components/icons";
import { useLibraryVersion, usePageScroller, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { moodMixName, partOfDay, resumeStart } from "../lib/home";
import { intent } from "../lib/ipc";
import {
  albumContext,
  artistContext,
  openRoute,
  playFrom,
  playLocal,
  playlistContext,
  playTracks,
  shuffleAll,
  useShuffleContext,
} from "../lib/play";
import type { HomeItem, Mood, MoodAlbum, SourceKind, Track } from "../lib/types";
import { nav, type Entry } from "../state/nav";
import { useOpenNowPlaying } from "../state/nowPlaying";
import { usePageSubject } from "../state/pageSubject";
import { usePalettes, usePaletteState } from "../state/palettes";
import { useAppState } from "../state/store";
import { useCanPlay } from "./common";

/** The local calendar day, `YYYY-MM-DD`. */
export function localDate(now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/** The time now, for relative times and the part of the day: once a minute. */
function useNow(): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 60_000);
    return () => window.clearInterval(timer);
  }, []);
  return now;
}

/** A track's id as `HomeStop.trackId` names it. */
function trackId(track: Track): string | null {
  return track.source.kind === "local" ? track.source.id : track.source.songId;
}

function openItem(item: HomeItem) {
  const { source, id, name } = item;
  switch (item.kind) {
    case "album":
      nav.push({ kind: "album", source, id, name });
      break;
    case "artist":
      nav.push({ kind: "artist", source, id, name });
      break;
    case "playlist":
      nav.push({ kind: "playlist", source, id, name });
      break;
    case "folder":
      nav.push({ kind: "folder", path: item.path ?? [] });
      break;
    case "mix":
      nav.push({ kind: "mixes", source });
      break;
  }
}

/** Plays `tracks` of `item` from where it stopped (Resume). */
function resumeTracks(item: HomeItem, tracks: Track[], context: Parameters<typeof playTracks>[3]) {
  const { start, resume } = resumeStart(item.stop, tracks.map(trackId));
  playTracks(tracks, start, false, context, resume);
}

function resumeItem(item: HomeItem) {
  const { source, id, name, coverUrl } = item;
  const stop = item.stop;
  // Local artists and folders are resolved by the engine, in the order
  // the stop was found in (the backend's `resolve_stop`).
  const localStart = () => ({
    start: stop?.index ?? 0,
    resume:
      stop?.trackId && stop.positionMs > 0
        ? { seekMs: stop.positionMs, trackId: stop.trackId }
        : null,
  });
  switch (item.kind) {
    case "album":
      playFrom(async () =>
        resumeTracks(
          item,
          (await browse.album(source, id)).tracks,
          albumContext(source, { id, name, artist: item.subtitle, coverUrl }),
        ),
      );
      break;
    case "playlist":
      playFrom(async () =>
        resumeTracks(
          item,
          (await browse.playlist(source, id)).tracks,
          playlistContext(source, { id, name, coverUrl }),
        ),
      );
      break;
    case "artist":
      if (source === "local") {
        const { start, resume } = localStart();
        playLocal({ kind: "artist", id }, false, start, resume);
      } else {
        playFrom(async () =>
          resumeTracks(
            item,
            await browse.navidromeArtistTracks(id),
            artistContext(source, { id, name, coverUrl }),
          ),
        );
      }
      break;
    case "folder": {
      const { start, resume } = localStart();
      playLocal({ kind: "folder", path: item.path ?? [] }, false, start, resume);
      break;
    }
    case "mix":
      intent({ type: "playMix", id });
      break;
  }
}

function playMood(source: SourceKind, mood: Mood) {
  intent({
    type: "playQuickMix",
    mix: { name: moodMixName(mood), source, ingredients: mood.albums.map((a) => a.ingredient) },
  });
}

export function HomeView({ entry, source }: { entry: Entry; source: SourceKind }) {
  const version = useLibraryVersion();
  const { snapshot } = useAppState();
  const historyRevision = snapshot?.historyRevision ?? 0;
  const now = useNow();
  const today = localDate(now);
  const canPlay = useCanPlay();
  const scroller = usePageScroller();
  const openNowPlaying = useOpenNowPlaying();

  // Local items resolve against the library (reload as it changes); both
  // sources reload when the history changes (a play, a stop).
  const history = useLoad(
    () => browse.homeHistory(source),
    `history:${source}`,
    source === "local" ? `${version}|${historyRevision}` : historyRevision,
  );
  // Local moods wait for the scan: picked over a growing album set they
  // would be the day's from a partial library. They reload once a scan
  // has finished (new library generation, so the shown ids stay valid).
  const waitForScan = source === "local" && snapshot?.scan.state === "scanning";
  const settled = `${version}|${snapshot?.browse.library ?? 0}`;
  const [moodsVersion, setMoodsVersion] = useState(settled);
  useEffect(() => {
    if (!waitForScan) setMoodsVersion(settled);
  }, [waitForScan, settled]);
  const moods = useLoad(
    () => (waitForScan ? Promise.resolve(null) : browse.moods(source, today)),
    `moods:${source}:${today}`,
    source === "local" ? moodsVersion : null,
  );
  useScroll(scroller, entry, history.state !== "loading");
  useShuffleContext(() => shuffleAll(source));

  const items = history.state === "ready" ? history.data.recent : [];
  // The focused item by key, so a reload (a new play) keeps it.
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const found = focusKey === null ? -1 : items.findIndex((i) => itemKey(i) === focusKey);
  const focus = found >= 0 ? found : 0;
  const focused = items[focus] ?? null;
  usePageSubject(entry, focused ? focused.coverUrl : history.state === "loading" ? undefined : null, {
    glow: false,
  });
  // Every carousel cover's palette, ahead: the page's glow and colours
  // start blending as soon as the focus moves (a glow layer waits for its
  // palette). The focused cover's first, in a batch of its own; the
  // others once it is known (or after a moment, should it not come).
  const focusedUrl = focused?.coverUrl ?? null;
  const focusedPalette = usePaletteState(focusedUrl);
  const [othersDue, setOthersDue] = useState(false);
  useEffect(() => {
    if (othersDue || items.length === 0) return;
    if (focusedPalette !== undefined) {
      setOthersDue(true);
      return;
    }
    const timer = window.setTimeout(() => setOthersDue(true), 1000);
    return () => window.clearTimeout(timer);
  }, [othersDue, focusedPalette, items.length]);
  usePalettes(
    othersDue ? items.map((i) => i.coverUrl).filter((u): u is string => u !== null && u !== focusedUrl) : [],
  );

  const moveFocus = useCallback(
    (index: number) => {
      const item = items[index];
      if (item) setFocusKey(itemKey(item));
    },
    [items],
  );

  const pc = snapshot?.playingContext ?? null;
  const playing = pc && pc.source === source ? `${pc.kind}:${pc.id}` : null;
  const fresh = history.state === "ready" && items.length === 0;

  const [showOther, setShowOther] = useState(false);
  const moodList =
    moods.state === "ready" && moods.data ? (showOther ? moods.data.other : moods.data.today) : null;
  const hasOther = moods.state === "ready" && (moods.data?.other.length ?? 0) > 0;

  const openAlbum = (album: MoodAlbum) =>
    nav.push({ kind: "album", source, id: album.id, name: album.name });

  const heading = (
    <div className="home-heading">
      <div className="eyebrow">{partOfDay(now)}</div>
      <h1 className="list-title">Where you left off</h1>
    </div>
  );
  const carousel = history.state === "ready" && !fresh;
  return (
    <section className="view home">
      {/* The carousel draws the heading itself, with its ‹ › buttons. */}
      {!carousel && <header className="home-header">{heading}</header>}
      <div className="home-left">
        {history.state === "error" ? (
          <p className="home-quiet secondary">Could not load your history: {history.error.message}</p>
        ) : fresh ? (
          <div className="home-welcome">
            <p className="home-welcome-text">
              Nothing played yet. What you listen to will wait for you here.
            </p>
            <div className="home-welcome-actions">
              <button
                className="button button-primary"
                disabled={!canPlay}
                onClick={() => shuffleAll(source)}
              >
                <ShuffleIcon size={14} /> Shuffle your library
              </button>
              <button
                className="button"
                onClick={() => {
                  nav.selectRoot("library");
                  nav.setSection("albums");
                }}
              >
                Browse albums
              </button>
            </div>
          </div>
        ) : history.state === "loading" ? (
          <div className="carousel is-loading" aria-busy="true" aria-label="Loading">
            <div className="carousel-track">
              <span className="skeleton carousel-skeleton" />
            </div>
            <div className="carousel-meta" />
          </div>
        ) : (
          <HomeCarousel
            heading={heading}
            items={items}
            focus={focus}
            onFocus={moveFocus}
            playing={playing}
            canPlay={canPlay}
            now={now}
            onOpen={openItem}
            onResume={resumeItem}
            onNowPlaying={openNowPlaying}
            onArtist={(item) =>
              item.artistId &&
              item.subtitle &&
              openRoute({ kind: "artist", source, id: item.artistId, name: item.subtitle })
            }
          />
        )}
      </div>

      <section className="home-moods section-rule" aria-labelledby="home-moods-title">
        <div className="home-moods-header">
          <h2 className="section-title" id="home-moods-title">
            {showOther ? "Other moods" : "Moods for today"}
          </h2>
          {hasOther && (
            <button className="button" onClick={() => setShowOther((v) => !v)}>
              {showOther ? "Today's moods" : "Other moods"}
            </button>
          )}
        </div>
        {moods.state === "error" ? (
          <p className="home-quiet secondary">
            {source === "navidrome" && moods.error.kind === "failed"
              ? "Navidrome is unreachable; today's moods will be here when it answers."
              : `Could not pick today's moods: ${moods.error.message}`}
          </p>
        ) : moodList && moodList.length === 0 ? (
          <p className="home-quiet secondary">
            No moods today: they come from your albums' genres.
          </p>
        ) : (
          <>
            {waitForScan && !moodList && (
              <p className="home-quiet secondary">
                Today's moods are picked once the library scan is done.
              </p>
            )}
            <div className="mood-grid" aria-busy={moodList ? undefined : true}>
              {moodList
                ? moodList.map((mood) => (
                    <MoodCard
                      key={mood.genre}
                      mood={mood}
                      source={source}
                      now={now}
                      canPlay={canPlay}
                      onPlay={(m) => playMood(source, m)}
                      onOpenAlbum={openAlbum}
                    />
                  ))
                : [0, 1, 2].map((i) => <div key={i} className="mood-card skeleton-card" />)}
            </div>
          </>
        )}
      </section>
    </section>
  );
}
