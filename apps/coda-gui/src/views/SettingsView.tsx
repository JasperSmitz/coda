// Settings: music folder (native picker), Navidrome server (test, save,
// disconnect; the lyrics index's status, Rebuild and switch), cover art
// and scrobbling, and Appearance (themes; the
// GUI's own settings.toml, see views/settings/AppearanceSection). Every change is saved to Coda's
// config.toml (shared with the TUI) and the keychain by the backend, then
// applied live.

import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { ServerFields, serverComplete } from "../components/ServerFields";
import { AppearanceSection } from "./settings/AppearanceSection";
import { errorText, lyricsIndexApi, onLyricsIndex, settingsApi } from "../lib/ipc";
import type { LyricsIndexStatus, ServerInput, Settings, Snapshot } from "../lib/types";

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

export function SettingsView({ snapshot }: { snapshot: Snapshot }) {
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
      <header className="view-header">
        <h1>Settings</h1>
      </header>
      <div className="view-body settings-body">
        {loadError && <p className="settings-status settings-status-error">{loadError}</p>}
        {settings && (
          <>
            {settings.problem && (
              <p className="settings-status settings-status-error">{settings.problem}</p>
            )}
            <MusicSection settings={settings} snapshot={snapshot} reload={reload} />
            <ServerSection settings={settings} snapshot={snapshot} reload={reload} />
            <OptionsSection settings={settings} reload={reload} />
            <AppearanceSection />
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

/** Settings › Navidrome › Lyrics search: status, Rebuild and the switch. */
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
