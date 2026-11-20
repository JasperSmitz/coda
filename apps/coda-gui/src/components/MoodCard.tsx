// A mood for today (DESIGN.md "Home"): three albums of one genre as a
// card, coloured by their covers (`moodSwatches`: two of their swatches,
// the vivid colours, in radial gradients over glass; until known, or for
// greyscale covers, the three tints as a wash),
// the covers fanned (they spread a little on hover; transforms only), the
// eyebrow, the genre in display type, the album titles, the facts and a
// round Play button in the first album's accent. Play starts a quick mix
// of the three albums; a click on a cover opens that album.

import type { CSSProperties } from "react";
import { moodEyebrow, moodFacts, moodSwatches } from "../lib/home";
import type { Mood, MoodAlbum, SourceKind } from "../lib/types";
import { accentStyle, usePalettes } from "../state/palettes";
import { useAccent, musicAccentOn } from "../state/accent";
import { morphKey } from "../lib/pageTransition";
import { Cover } from "./Cover";
import { PlayIcon } from "./icons";

export function MoodCard({
  mood,
  source,
  now,
  canPlay,
  onPlay,
  onOpenAlbum,
}: {
  mood: Mood;
  source: SourceKind;
  now: Date;
  canPlay: boolean;
  onPlay: (mood: Mood) => void;
  onOpenAlbum: (album: MoodAlbum) => void;
}) {
  const urls = mood.albums.map((a) => a.coverUrl).filter((u): u is string => u !== null);
  const palettes = usePalettes(urls);
  const { shown } = useAccent();
  const music = musicAccentOn(shown);
  const tint = (i: number) => {
    const url = mood.albums[i]?.coverUrl;
    return (url && palettes.get(url)?.tint) || null;
  };
  const swatches = moodSwatches(
    mood.albums.map((a) => (a.coverUrl ? (palettes.get(a.coverUrl)?.swatches ?? []) : [])),
  );
  // The card's colours: two swatches; without any, each album's tint
  // (the base where one is unknown).
  const colours: Record<string, string> = swatches
    ? { "--mood-swatch-1": swatches[0], "--mood-swatch-2": swatches[1] }
    : {
        "--mood-tint-1": tint(0) ?? "var(--bg)",
        "--mood-tint-2": tint(1) ?? tint(0) ?? "var(--bg)",
        "--mood-tint-3": tint(2) ?? tint(1) ?? tint(0) ?? "var(--bg)",
      };
  const first = mood.albums[0]?.coverUrl;
  const firstPalette = first ? (palettes.get(first) ?? null) : null;
  const accent = music && firstPalette?.vivid ? firstPalette : null;
  const eyebrow = moodEyebrow(mood, source);
  const facts = moodFacts(mood, now);
  return (
    <article
      className={`mood-card ${swatches ? "has-swatches" : ""}`}
      style={colours as CSSProperties}
      aria-label={`${mood.genre} mood`}
    >
      <div className="mood-fan" data-count={mood.albums.length}>
        {mood.albums.map((album, i) => (
          <button
            key={album.id}
            className={`mood-fan-cover mood-fan-${i + 1}`}
            title={`Open ${album.name}`}
            aria-label={`Open album ${album.name}`}
            onClick={() => onOpenAlbum(album)}
          >
            <Cover
              url={album.coverUrl}
              name={album.name}
              size="small"
              className="mood-cover"
              morph={morphKey("album", source, album.id)}
            />
          </button>
        ))}
      </div>
      <div className="mood-text">
        {eyebrow && <div className="eyebrow mood-eyebrow">{eyebrow}</div>}
        <h3 className="mood-genre" title={mood.genre}>
          {mood.genre}
        </h3>
        <ul className="mood-albums">
          {mood.albums.map((album) => (
            <li key={album.id} title={album.artist ? `${album.name} · ${album.artist}` : album.name}>
              {album.name}
              {album.artist && <span className="mood-album-artist"> · {album.artist}</span>}
            </li>
          ))}
        </ul>
        <div className="mood-foot">
          <p className="mood-facts">{facts}</p>
          <button
            className="mood-play"
            style={accentStyle(accent)}
            disabled={!canPlay}
            aria-label={`Play a quick mix of ${mood.genre}`}
            title="Play a quick mix of these albums"
            onClick={() => onPlay(mood)}
          >
            <PlayIcon size={18} />
          </button>
        </div>
      </div>
    </article>
  );
}
