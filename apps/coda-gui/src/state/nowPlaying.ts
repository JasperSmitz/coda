// Opening Now Playing from a view (Home's "Now Playing" on the playing
// context): App provides its opener, which morphs the player's cover into
// Now Playing as the player's cover button does.

import { createContext, useContext } from "react";

export const NowPlayingOpener = createContext<() => void>(() => {});

/** Opens Now Playing (windowed). */
export function useOpenNowPlaying(): () => void {
  return useContext(NowPlayingOpener);
}
