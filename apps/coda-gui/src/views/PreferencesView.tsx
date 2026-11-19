// Preferences ("Make it yours"), glass cards in this order: the accent
// (from the music, the default; curated swatches or a custom colour; the
// GUI's own settings.toml, see state/accent), the music source (the Local | Navidrome switch,
// Ctrl/Cmd+1/2, and its status), Library (Covers or List by default,
// settings.toml too, see state/libraryView), music folder (native picker), Navidrome
// server (test, save, disconnect; the lyrics index's status, Rebuild and
// switch), cover art and scrobbling. Those after the accent are saved to
// Coda's config.toml (shared with the TUI) and the keychain by the
// backend, then applied live.

import { useCallback, useEffect, useRef, useState, type FormEvent, type KeyboardEvent, type ReactNode } from "react";
import { ServerFields, serverComplete } from "../components/ServerFields";
import { GridIcon, ListIcon, LocalIcon, ServerIcon } from "../components/icons";
import { bothConfigured, switchSource, useSources } from "../state/source";
import { accent, useAccent } from "../state/accent";
import { chooseLibraryDefault, useLibraryDefault } from "../state/libraryView";
import { HOUSE, MIN_ACCENT_CONTRAST, normalizeHex } from "../lib/accent";
import { MUSIC } from "../lib/palette";
import { isMacos } from "../lib/platform";
import { errorText, lyricsIndexApi, onLyricsIndex, settingsApi } from "../lib/ipc";
import { usePageScroller, useScroll } from "../hooks/useLayout";
import type { Entry } from "../state/nav";
import type {
  LibraryView,
  LyricsIndexStatus,
  ServerInput,
  Settings,
  Snapshot,
  SourceKind,
} from "../lib/types";

/** A second click within this time disconnects. */
const DISCONNECT_CONFIRM_MS = 3000;

type Status = { kind: "idle" } | { kind: "busy"; text: string } | { kind: "done"; text: string } | { kind: "error"; text: string };

function useAction(reload: () => void) {
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const run = useCallback(
    (busy: string, done: string, work: () => Promise<unknown>, after?: () => void) => {
      setStatus({ kind: "busy", text: busy });
      work()
        .then(() => {
          setStatus({ kind: "done", text: done });
          after?.();
        })
        .catch((e: unknown) => setStatus({ kind: "error", text: errorText(e) }))
        .finally(reload);
    },
    [reload],
  );
  return { status, run, busy: status.kind === "busy" };
}

function StatusLine({ status }: { status: Status }) {
  if (status.kind === "idle") return null;
  return (
    <p role="status" className={`settings-status settings-status-${status.kind}`}>
      {status.text}
    </p>
  );
}

const CONNECTION_TEXT = {
  notConfigured: "Not connected",
  connecting: "Connecting…",
  connected: "Connected",
  failed: "Not reachable",
} as const;

export function PreferencesView({ entry, snapshot }: { entry: Entry; snapshot: Snapshot }) {
  // The page starts at its top, and keeps its place while shown.
  useScroll(usePageScroller(), entry, true);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const reload = useCallback(() => {
    settingsApi
      .get()
      .then((s) => {
        setSettings(s);
        setLoadError(null);
      })
      .catch((e: unknown) => setLoadError(errorText(e)));
  }, []);
  useEffect(reload, [reload]);

  return (
    <section className="view settings">
      <header className="list-header">
        <div className="eyebrow">Preferences</div>
        <h1 className="list-title">Make it yours</h1>
      </header>
      <div className="view-body settings-body">
        {loadError && <p className="settings-status settings-status-error">{loadError}</p>}
        {settings && (
          <>
            {settings.problem && (
              <p className="settings-status settings-status-error">{settings.problem}</p>
            )}
            <AccentSection />
            <SourceSection snapshot={snapshot} />
            <LibrarySection />
            <MusicSection settings={settings} snapshot={snapshot} reload={reload} />
            <ServerSection settings={settings} snapshot={snapshot} reload={reload} />
            <OptionsSection settings={settings} reload={reload} />
            {settings.configIsLink && (
              <p className="settings-status settings-status-error">
                The config file is a symbolic link. Saving here replaces the link with a regular
                file; the file it points to is left as it is.
              </p>
            )}
            {settings.configPath && (
              <p className="settings-footnote secondary">
                Saved in <span className="mono">{settings.configPath}</span>, shared with the Coda
                terminal app. Comments in it are not kept; the first save of a hand-edited file
                keeps a backup next to it.
              </p>
            )}
          </>
        )}
      </div>
    </section>
  );
}

/** Arrow keys among a radio group's options (not the global seek). */
function radioArrow(e: KeyboardEvent): -1 | 1 | 0 {
  if (e.key === "ArrowLeft" || e.key === "ArrowUp") return -1;
  if (e.key === "ArrowRight" || e.key === "ArrowDown") return 1;
  return 0;
}

/**
 * The accent: "From the music" (the default: pages, the player and the
 * queue take their covers' colours), six curated swatches (the first is
 * the house amber) and a custom colour (hex field and the native picker).
 * One radio group: music first, then the swatches. Applied and saved at
 * once; a custom colour under 3:1 on the base gets a note.
 */
function AccentSection() {
  const { prefs, shown, error } = useAccent();
  const [draft, setDraft] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  // The colour picker's own value while it is used: React never writes an
  // older value back into it (WebKit would push that to the open native
  // picker and make it jump back mid-drag).
  const [picking, setPicking] = useState<string | null>(null);
  if (!prefs || !shown) {
    return (
      <section className="settings-section" aria-labelledby="settings-accent">
        <h2 id="settings-accent">Accent</h2>
        {error && <p className="settings-status settings-status-error">{error}</p>}
      </section>
    );
  }
  // What is shown is the latest choice, even while it is being saved.
  const music = shown === MUSIC;
  const curated = prefs.swatches.find((s) => s.value === shown);
  const custom = !curated && !music;
  // From the music: the picker starts at the house colour.
  const shownColor = curated ? curated.color : music ? prefs.swatches[0].color : shown;
  const chosenName = music ? "From the music" : curated ? curated.name : `Custom ${shownColor}`;
  /** The radio group's options: music, then the swatches. */
  const options = [MUSIC, ...prefs.swatches.map((s) => s.value)];
  const choose = (value: string) => {
    setDraft(null);
    setInvalid(false);
    accent.choose(value);
  };
  const commitDraft = () => {
    if (draft === null) return;
    if (draft.trim() === "") {
      setDraft(null);
      return;
    }
    const hex = normalizeHex(draft);
    if (!hex) {
      setInvalid(true);
      return;
    }
    choose(hex);
  };
  // Roving focus: arrows move to and choose the next option.
  const onKeyDown = (e: KeyboardEvent) => {
    const step = radioArrow(e);
    if (step === 0) return;
    e.preventDefault();
    e.stopPropagation();
    const at = options.indexOf(shown);
    const index = at >= 0 ? at : step > 0 ? -1 : 0;
    const count = options.length;
    const next = options[(index + step + count) % count];
    choose(next);
    requestAnimationFrame(() =>
      document.querySelector<HTMLElement>(`.accent-swatches [data-value="${next}"]`)?.focus(),
    );
  };
  return (
    <section className="settings-section" aria-labelledby="settings-accent">
      <h2 id="settings-accent">Accent</h2>
      <div className="accent-row">
        <div
          className="accent-swatches"
          role="radiogroup"
          aria-labelledby="settings-accent"
          onKeyDown={onKeyDown}
        >
          <button
            role="radio"
            aria-checked={music}
            aria-describedby="accent-music-note"
            data-value={MUSIC}
            tabIndex={music || custom ? 0 : -1}
            className={`accent-music ${music ? "is-active" : ""}`}
            onClick={() => choose(MUSIC)}
          >
            <span className="accent-music-dot" aria-hidden="true" />
            From the music
          </button>
          {prefs.swatches.map((swatch) => {
            const checked = swatch === curated;
            return (
              <button
                key={swatch.value}
                role="radio"
                aria-checked={checked}
                aria-label={swatch.name}
                title={swatch.value === HOUSE ? `${swatch.name} (Coda’s own)` : swatch.name}
                data-value={swatch.value}
                tabIndex={checked ? 0 : -1}
                className={`accent-swatch ${checked ? "is-active" : ""}`}
                style={{ background: swatch.color }}
                onClick={() => choose(swatch.value)}
              />
            );
          })}
        </div>
        <span className="accent-name">{chosenName}</span>
      </div>
      <p id="accent-music-note" className="secondary accent-note">
        A page takes the colours of the album it shows, the player those of the song that’s
        playing.
      </p>
      <div className="accent-custom">
        <label htmlFor="accent-hex">Custom colour</label>
        <input
          id="accent-hex"
          type="text"
          className={invalid ? "is-invalid" : ""}
          spellCheck={false}
          autoComplete="off"
          maxLength={7}
          placeholder="#rrggbb"
          aria-invalid={invalid}
          aria-describedby={invalid ? "accent-hex-error" : undefined}
          value={draft ?? (custom ? shownColor : "")}
          onChange={(e) => {
            setDraft(e.target.value);
            setInvalid(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") commitDraft();
            if (e.key === "Escape" && draft !== null) {
              // Back to the saved value, not the global Esc (Back).
              e.stopPropagation();
              setDraft(null);
              setInvalid(false);
            }
          }}
          onBlur={commitDraft}
        />
        <input
          type="color"
          className="accent-picker"
          aria-label="Pick a custom colour"
          title="Pick a custom colour"
          value={picking ?? shownColor}
          onChange={(e) => {
            setPicking(e.target.value);
            choose(e.target.value);
          }}
          onBlur={() => setPicking(null)}
        />
      </div>
      {invalid && (
        <p id="accent-hex-error" className="settings-status settings-status-error">
          Type a colour as #rrggbb.
        </p>
      )}
      {custom && prefs.accent === shown && prefs.accentContrast < MIN_ACCENT_CONTRAST && (
        <p className="secondary accent-note">
          This colour is hard to see on the dark background (contrast{" "}
          {prefs.accentContrast.toFixed(1)}:1; 3:1 or more reads well). It is used as you chose it.
        </p>
      )}
      {error && <p className="settings-status settings-status-error">{error}</p>}
    </section>
  );
}

const SOURCE_NAME: Record<SourceKind, string> = { local: "Local", navidrome: "Navidrome" };
/** Ctrl on Linux, Cmd on macOS (both work). */
const MOD = isMacos ? "⌘" : "Ctrl+";

/** The local scan's state in a few words (`null`: no library). */
function scanStatus(snapshot: Snapshot): string | null {
  const { scan } = snapshot;
  switch (scan.state) {
    case "noLibrary":
      return null;
    case "scanning":
      return `Scanning… ${scan.tracks.toLocaleString()}`;
    case "finished":
      return `${scan.tracks.toLocaleString()} tracks`;
    case "failed":
      return "Scan failed";
  }
}

/** A source's status: the scan line, or the connection state with its dot. */
function SourceStatus({ source, snapshot }: { source: SourceKind; snapshot: Snapshot }) {
  if (source === "local") return <span className="source-status-text">{scanStatus(snapshot)}</span>;
  const state = snapshot.navidrome.state;
  return (
    <span className="source-status-text">
      <span className={`connection connection-${state}`} aria-hidden="true" />
      {CONNECTION_TEXT[state]}
    </span>
  );
}

/**
 * The source browsed (one at a time): a switch when both are set up,
 * else the one source's name. Ctrl/Cmd+1 and +2 switch from anywhere.
 */
function SourceSection({ snapshot }: { snapshot: Snapshot }) {
  const sources = useSources();
  const source = sources.active;
  const option = (kind: SourceKind, index: number, icon: ReactNode) => (
    <button
      role="radio"
      aria-checked={source === kind}
      tabIndex={source === kind ? 0 : -1}
      className={`source-option ${source === kind ? "is-active" : ""}`}
      onClick={() => switchSource(kind)}
      title={`Browse ${kind === "local" ? "the local library" : "Navidrome"} (${MOD}${index})`}
    >
      {icon}
      <span>{SOURCE_NAME[kind]}</span>
    </button>
  );
  const onKeyDown = (e: KeyboardEvent) => {
    const keys = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"];
    if (!keys.includes(e.key)) return;
    // Not a seek (the global ←/→).
    e.preventDefault();
    e.stopPropagation();
    const next: SourceKind = source === "local" ? "navidrome" : "local";
    switchSource(next);
    // The switch re-renders in place: keep the focus on the chosen option.
    requestAnimationFrame(() =>
      document.querySelector<HTMLElement>('.source-switch [aria-checked="true"]')?.focus(),
    );
  };
  return (
    <section className="settings-section" aria-labelledby="settings-source">
      <h2 id="settings-source">Music source</h2>
      {bothConfigured(sources) ? (
        <>
          <div
            className="source-switch"
            role="radiogroup"
            aria-labelledby="settings-source"
            onKeyDown={onKeyDown}
          >
            {option("local", 1, <LocalIcon size={15} />)}
            {option("navidrome", 2, <ServerIcon size={15} />)}
          </div>
          <p className="secondary source-status">
            <span>{source === "local" ? "Local library" : "Navidrome server"}</span>
            <SourceStatus source={source} snapshot={snapshot} />
          </p>
        </>
      ) : (
        <p className="secondary source-status">
          <span>{SOURCE_NAME[source]}</span>
          <SourceStatus source={source} snapshot={snapshot} />
        </p>
      )}
    </section>
  );
}

/**
 * Library's default view: Covers or List for albums and artists (a
 * section switched in Library keeps its own choice). Shown at once,
 * saved to settings.toml.
 */
function LibrarySection() {
  const { view, error } = useLibraryDefault();
  const option = (kind: LibraryView, label: string, icon: ReactNode) => (
    <button
      role="radio"
      aria-checked={view === kind}
      tabIndex={view === kind ? 0 : -1}
      className={`source-option ${view === kind ? "is-active" : ""}`}
      onClick={() => chooseLibraryDefault(kind)}
    >
      {icon}
      <span>{label}</span>
    </button>
  );
  const onKeyDown = (e: KeyboardEvent) => {
    if (radioArrow(e) === 0) return;
    // Not a seek (the global ←/→).
    e.preventDefault();
    e.stopPropagation();
    chooseLibraryDefault(view === "covers" ? "list" : "covers");
    requestAnimationFrame(() =>
      document
        .querySelector<HTMLElement>('.library-view-switch [aria-checked="true"]')
        ?.focus(),
    );
  };
  return (
    <section className="settings-section" aria-labelledby="settings-library">
      <h2 id="settings-library">Library</h2>
      <p className="secondary">Show albums and artists as</p>
      <div
        className="source-switch library-view-switch"
        role="radiogroup"
        aria-labelledby="settings-library"
        onKeyDown={onKeyDown}
      >
        {option("covers", "Covers", <GridIcon size={15} />)}
        {option("list", "List", <ListIcon size={15} />)}
      </div>
      <p className="secondary">
        A section you switch in Library keeps its own choice.
      </p>
      {error && <p className="settings-status settings-status-error">{error}</p>}
    </section>
  );
}

function MusicSection({ settings, snapshot, reload }: { settings: Settings; snapshot: Snapshot; reload: () => void }) {
  const { status, run, busy } = useAction(reload);
  const choose = () => {
    settingsApi
      .pickFolder()
      .then((folder) => {
        if (folder) run("Saving…", "Music folder saved; scanning", () => settingsApi.setMusicDir(folder));
      })
      .catch(() => {});
  };
  const { scan } = snapshot;
  const scanText =
    scan.state === "scanning"
      ? `Scanning… ${scan.tracks.toLocaleString()} tracks`
      : scan.state === "finished"
        ? `${scan.tracks.toLocaleString()} tracks, ${scan.playlists.toLocaleString()} playlists`
        : scan.state === "failed"
          ? "The last scan failed"
          : null;
  return (
    <section className="settings-section" aria-labelledby="settings-music">
      <h2 id="settings-music">Music folder</h2>
      <div className="settings-row">
        <div>
          <div className="mono">{settings.musicDir ?? "Not set"}</div>
          {scanText && <div className="secondary">{scanText}</div>}
        </div>
        <button className="button" onClick={choose} disabled={busy || settings.musicDirEnv || settings.problem != null}>
          {settings.musicDir ? "Change…" : "Choose…"}
        </button>
      </div>
      {settings.musicDirEnv && (
        <p className="secondary">CODA_MUSIC_DIR is set and is used instead; unset it to change the folder here.</p>
      )}
      <StatusLine status={status} />
    </section>
  );
}

const EMPTY_SERVER: ServerInput = { url: "", username: "", password: "" };

function ServerSection({ settings, snapshot, reload }: { settings: Settings; snapshot: Snapshot; reload: () => void }) {
  const { status, run, busy } = useAction(reload);
  const [form, setForm] = useState<ServerInput | null>(null);
  const [armed, setArmed] = useState(false);
  const armTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(armTimer.current), []);
  const { server } = settings;
  const blocked = settings.serverBlockedBy.length > 0;
  const canWrite = !blocked && server.kind !== "outside" && settings.problem == null;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!form || !serverComplete(form)) return;
    const value = form;
    run("Connecting and saving…", "Server saved", () => settingsApi.saveServer(value), () => setForm(null));
  };
  const disconnect = () => {
    if (!armed) {
      setArmed(true);
      armTimer.current = window.setTimeout(() => setArmed(false), DISCONNECT_CONFIRM_MS);
      return;
    }
    window.clearTimeout(armTimer.current);
    setArmed(false);
    run("Disconnecting…", "Disconnected from Navidrome", () => settingsApi.disconnect());
  };

  return (
    <section className="settings-section" aria-labelledby="settings-server">
      <h2 id="settings-server">Navidrome</h2>
      {server.kind !== "notConfigured" && !form && (
        <div className="settings-row">
          <div>
            <div>
              {server.username ?? "?"} <span className="secondary">@</span>{" "}
              <span className="mono">{server.url ?? "?"}</span>
            </div>
            <div className="secondary">
              <span className={`connection connection-${snapshot.navidrome.state}`} />{" "}
              {CONNECTION_TEXT[snapshot.navidrome.state]}
              {snapshot.navidrome.state === "failed" && snapshot.navidrome.message
                ? `: ${snapshot.navidrome.message}`
                : ""}
            </div>
          </div>
          {canWrite && (
            <div className="settings-actions">
              <button
                className="button"
                onClick={() =>
                  setForm({
                    url: server.url ?? "",
                    username: server.username ?? "",
                    password: "",
                  })
                }
                disabled={busy}
              >
                Edit…
              </button>
              <button className={`button ${armed ? "button-danger" : ""}`} onClick={disconnect} disabled={busy}>
                {armed ? "Click again to disconnect" : "Disconnect"}
              </button>
            </div>
          )}
        </div>
      )}
      {server.kind === "notConfigured" && !form && (
        <div className="settings-row">
          <div className="secondary">No server is connected.</div>
          <button className="button" onClick={() => setForm(EMPTY_SERVER)} disabled={busy || !canWrite}>
            Connect…
          </button>
        </div>
      )}
      {server.kind === "outside" && (
        <p className="secondary">
          Set up outside Coda ({server.by.join(", ")}); change it where it is set.
        </p>
      )}
      {blocked && (
        <p className="secondary">
          {settings.serverBlockedBy.join(" and ")} {settings.serverBlockedBy.length === 1 ? "is" : "are"} set
          and would be used instead; unset {settings.serverBlockedBy.length === 1 ? "it" : "them"} to change the
          server here.
        </p>
      )}
      {server.kind !== "notConfigured" && !form && <LyricsIndexBlock />}
      {form && (
        <form className="settings-form" onSubmit={submit}>
          <ServerFields value={form} onChange={setForm} disabled={busy} autoFocus />
          <div className="settings-actions">
            <button type="button" className="button" onClick={() => setForm(null)} disabled={busy}>
              Cancel
            </button>
            <button type="submit" className="button button-primary" disabled={busy || !serverComplete(form)}>
              Save
            </button>
          </div>
        </form>
      )}
      <StatusLine status={status} />
    </section>
  );
}

const PAUSE_TEXT = {
  unreachable: "Paused: couldn’t reach the server",
  rejected: "Paused: the server rejected the login",
  failed: "Paused: the server’s answers could not be used",
} as const;

/** The index's status line, or null when there is nothing to say. */
function lyricsIndexText(s: LyricsIndexStatus): string | null {
  const songs = (n: number) => `${n.toLocaleString()} song${n === 1 ? "" : "s"}`;
  switch (s.state) {
    case "off":
    case "noServer":
      return null;
    case "loading":
      return "Loading the index…";
    case "idle":
      return `${songs(s.checked)} indexed`;
    case "albums":
      return s.todo > 0
        ? `Indexing… ${s.done.toLocaleString()} of ${s.todo.toLocaleString()} albums`
        : "Indexing… listing albums";
    case "songs":
      // Of all songs, as the search page's note counts them.
      return `Indexing… ${s.checked.toLocaleString()} of ${songs(s.songs)}`;
    case "upToDate":
      return `${songs(s.checked)} indexed · up to date`;
    case "paused":
      return `${PAUSE_TEXT[s.pause ?? "unreachable"]} · ${songs(s.checked)} indexed`;
  }
}

/** What the live region announces: the state, not every count. */
const STATE_ANNOUNCEMENT: Record<LyricsIndexStatus["state"], string> = {
  off: "Lyrics search off",
  noServer: "",
  loading: "Loading the lyrics index",
  idle: "Lyrics index loaded",
  albums: "Indexing lyrics",
  songs: "Indexing lyrics",
  upToDate: "Lyrics index up to date",
  paused: "Lyrics indexing paused",
};

/** Preferences › Navidrome › Lyrics search: status, Rebuild and the switch. */
function LyricsIndexBlock() {
  const [status, setStatus] = useState<LyricsIndexStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    const unlisten = onLyricsIndex((s) => live && setStatus(s));
    lyricsIndexApi
      .status()
      .then((s) => live && setStatus((current) => current ?? s))
      .catch((e: unknown) => live && setError(errorText(e)));
    return () => {
      live = false;
      void unlisten.then((stop) => stop());
    };
  }, []);
  if (!status || status.state === "noServer") return null;
  const enabled = status.state !== "off";
  const text = lyricsIndexText(status);
  const act = (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    work()
      .catch((e: unknown) => setError(errorText(e)))
      .finally(() => setBusy(false));
  };
  const syncing = status.state === "albums" || status.state === "songs" || status.state === "loading";
  return (
    <div className="settings-subsection" role="group" aria-labelledby="settings-lyrics-index">
      <h3 id="settings-lyrics-index">Lyrics search</h3>
      <label className="settings-check">
        <input
          type="checkbox"
          checked={enabled}
          disabled={busy}
          onChange={(e) => {
            const on = e.target.checked;
            // The answer is the new status, so the switch never snaps back.
            act(() => lyricsIndexApi.setEnabled(on).then(setStatus));
          }}
        />
        <span>
          Index lyrics for search
          <span className="secondary"> — kept on this computer, so Navidrome lyrics can be searched</span>
        </span>
      </label>
      {enabled && (
        <div className="settings-row">
          {/* The counts change up to 4 times a second: not live. */}
          <div className="secondary" aria-live="off">
            {text}
          </div>
          <button className="button" onClick={() => act(lyricsIndexApi.rebuild)} disabled={busy || syncing}>
            Rebuild index
          </button>
        </div>
      )}
      <span className="visually-hidden" role="status">
        {STATE_ANNOUNCEMENT[status.state]}
      </span>
      {error && <p className="settings-status settings-status-error">{error}</p>}
    </div>
  );
}

function OptionsSection({ settings, reload }: { settings: Settings; reload: () => void }) {
  const { status, run, busy } = useAction(reload);
  const disabled = busy || settings.problem != null;
  return (
    <section className="settings-section" aria-labelledby="settings-options">
      <h2 id="settings-options">Options</h2>
      <label className="settings-check">
        <input
          type="checkbox"
          checked={settings.coverArt}
          disabled={disabled}
          onChange={(e) => {
            const coverArt = e.target.checked;
            run("Saving…", coverArt ? "Cover art on" : "Cover art off", () => settingsApi.setOptions({ coverArt }));
          }}
        />
        <span>
          Show cover art
          <span className="secondary"> — from tags, folder images and Navidrome</span>
        </span>
      </label>
      <label className="settings-check">
        <input
          type="checkbox"
          checked={settings.scrobble}
          disabled={disabled}
          onChange={(e) => {
            const scrobble = e.target.checked;
            run("Saving…", scrobble ? "Scrobbling on" : "Scrobbling off", () => settingsApi.setOptions({ scrobble }));
          }}
        />
        <span>
          Report plays to Navidrome
          <span className="secondary"> — scrobbling, for Navidrome tracks</span>
        </span>
      </label>
      <StatusLine status={status} />
    </section>
  );
}
