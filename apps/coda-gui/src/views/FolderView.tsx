// The local folder tree, one level at a time: sub-folders, then the
// folder's own tracks. Enter on a track plays the folder's tracks from
// it; `s` (or Shuffle) plays everything below the folder shuffled.

import { Empty, ErrorState, SkeletonRows } from "../components/LoadState";
import { TrackTable } from "../components/TrackTable";
import { FolderIcon } from "../components/icons";
import { useLibraryVersion, usePageScroller, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { browse } from "../lib/browse";
import { folderContext, playLocal, useShuffleContext } from "../lib/play";
import { nav, useNav, type Entry } from "../state/nav";
import {
  ListHeader,
  PageActions,
  PageHeader,
  PlayButtons,
  ScanLine,
  SOURCE_LABEL,
  plural,
} from "./common";

export function FolderView({ entry, path }: { entry: Entry; path: string[] }) {
  const version = useLibraryVersion();
  const load = useLoad(() => browse.folder(path), JSON.stringify(path), version);
  const scroller = usePageScroller();
  const folder = load.data;
  useScroll(scroller, entry, folder !== undefined, load.state !== "loading");

  // Everything below the folder, resolved by the engine (a whole-library
  // folder never crosses IPC).
  const playAll = (shuffle: boolean) => playLocal({ kind: "folder", path }, shuffle);
  const hasContent = folder !== undefined && (folder.dirs.length > 0 || folder.tracks.length > 0);
  useShuffleContext(hasContent ? () => playAll(true) : null);

  // "Music": back to the start of the stack if it is the folder root
  // (Library's Folders), else a new page.
  const stack = useNav();
  const root = stack[0].route;
  const openMusicRoot = () => {
    if (root.kind === "folder" && root.path.length === 0) nav.toRoot();
    else nav.push({ kind: "folder", path: [] });
  };

  const title = path.length > 0 ? path[path.length - 1] : "Folders";
  const counts =
    folder &&
    [
      folder.dirs.length > 0 ? plural(folder.dirs.length, "folder") : null,
      folder.tracks.length > 0 ? plural(folder.tracks.length, "song") : null,
    ]
      .filter(Boolean)
      .join(" · ");
  return (
    <section className="view">
      {path.length === 0 ? (
        <ListHeader
          eyebrow={SOURCE_LABEL.local}
          title={title}
          meta={counts}
          tools={
            <PlayButtons
              onPlay={() => playAll(false)}
              onShuffle={() => playAll(true)}
              disabled={!hasContent}
            />
          }
        />
      ) : (
        <PageHeader
          eyebrow="Folder"
          title={title}
          byline={
            <nav className="breadcrumbs" aria-label="Folder path">
              <button className="link-button" onClick={openMusicRoot}>
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
          }
          meta={counts}
          actions={
            <PageActions
              onPlay={() => playAll(false)}
              onShuffle={() => playAll(true)}
              disabled={!hasContent}
            />
          }
        />
      )}
      <ScanLine />
      <div className="view-body">
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
            context={folderContext(path)}
          />
        )}
      </div>
    </section>
  );
}
