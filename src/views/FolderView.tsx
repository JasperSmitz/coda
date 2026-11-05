// The local folder tree, one level at a time: sub-folders, then the
// folder's own tracks. Enter on a track plays the folder's tracks from
// it; `s` (or Shuffle) plays everything below the folder shuffled.

import { useRef } from "react";
import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { TrackTable } from "../components/TrackTable";
import { FolderIcon } from "../components/icons";
import { useLibraryVersion, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { playLocal, useShuffleContext } from "../lib/play";
import { nav, type Entry } from "../state/nav";
import { PlayButtons, ScanLine, SOURCE_LABEL, ViewHeader, plural } from "./common";

export function FolderView({ entry, path }: { entry: Entry; path: string[] }) {
  const version = useLibraryVersion();
  const load = useLoad(() => browse.folder(path), JSON.stringify(path), version);
  const scroller = useRef<HTMLDivElement>(null);
  const folder = load.data;
  useScroll(scroller, entry, folder !== undefined);

  // Everything below the folder, resolved by the engine (a whole-library
  // folder never crosses IPC).
  const playAll = (shuffle: boolean) => playLocal({ kind: "folder", path }, shuffle);
  const hasContent = folder !== undefined && (folder.dirs.length > 0 || folder.tracks.length > 0);
  useShuffleContext(hasContent ? () => playAll(true) : null);

  const title = path.length > 0 ? path[path.length - 1] : "Folders";
  return (
    <section className="view">
      <ViewHeader
        eyebrow={SOURCE_LABEL.local}
        title={title}
        actions={
          <PlayButtons
            onPlay={() => playAll(false)}
            onShuffle={() => playAll(true)}
            disabled={!hasContent}
          />
        }
      >
        {path.length > 0 && (
          <nav className="breadcrumbs" aria-label="Folder path">
            <button className="link-button" onClick={() => nav.root({ kind: "folder", path: [] })}>
              Music
            </button>
            {path.slice(0, -1).map((name, i) => (
              <span key={i}>
                <span className="crumb-sep">/</span>
                <button
                  className="link-button"
                  onClick={() => nav.push({ kind: "folder", path: path.slice(0, i + 1) })}
                >
                  {name}
                </button>
              </span>
            ))}
          </nav>
        )}
        {folder && (
          <div className="secondary">
            {[
              folder.dirs.length > 0 ? plural(folder.dirs.length, "folder") : null,
              folder.tracks.length > 0 ? plural(folder.tracks.length, "track") : null,
            ]
              .filter(Boolean)
              .join(" · ")}
          </div>
        )}
      </ViewHeader>
      <ScanLine />
      <div className="view-body" ref={scroller}>
        {load.state === "loading" && <SkeletonRows />}
        {load.state === "error" && <ErrorState error={load.error} onRetry={load.retry} />}
        {folder && !hasContent && (
          <Empty title="Empty folder">Nothing has been scanned here yet.</Empty>
        )}
        {folder && folder.dirs.length > 0 && (
          <div className="folder-grid" role="list" aria-label="Folders">
            {folder.dirs.map((name) => (
              <button
                key={name}
                role="listitem"
                className="folder-item"
                onClick={() => nav.push({ kind: "folder", path: [...path, name] })}
              >
                <FolderIcon />
                <span>{name}</span>
              </button>
            ))}
          </div>
        )}
        {folder && folder.tracks.length > 0 && (
          <TrackTable
            tracks={folder.tracks}
            scroller={scroller}
            showArtist
            showAlbum
            label={`Tracks in ${title}`}
          />
        )}
      </div>
    </section>
  );
}
