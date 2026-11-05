// First run (no music folder and no server configured): pick a music
// folder and/or connect a Navidrome server, then continue. Saved like
// `coda --setup`; the engine starts using it at once and the app shell
// replaces this screen.

import { useEffect, useState, type FormEvent } from "react";
import { ServerFields, serverComplete } from "../components/ServerFields";
import { errorText, settingsApi } from "../lib/ipc";
import type { ServerInput, Settings } from "../lib/types";

export function Onboarding() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [folder, setFolder] = useState<string | null>(null);
  const [connect, setConnect] = useState(false);
  const [server, setServer] = useState<ServerInput>({ url: "", username: "", password: "" });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    settingsApi
      .get()
      .then(setSettings)
      .catch((e: unknown) => setError(errorText(e)));
  }, []);

  const serverBlocked = settings != null && settings.serverBlockedBy.length > 0;
  const useServer = connect && !serverBlocked;
  const ready = (folder != null || useServer) && (!useServer || serverComplete(server));

  const choose = () => {
    settingsApi
      .pickFolder()
      .then((picked) => {
        if (picked) setFolder(picked);
      })
      .catch((e: unknown) => setError(errorText(e)));
  };

  const finish = (e: FormEvent) => {
    e.preventDefault();
    if (!ready || busy) return;
    setBusy(true);
    setError(null);
    settingsApi
      .finishOnboarding(folder, useServer ? server : null)
      // On success the snapshot loses its `noConfig` problem and the app
      // shell takes over.
      .catch((err: unknown) => setError(errorText(err)))
      .finally(() => setBusy(false));
  };

  return (
    <main className="onboarding">
      <form className="onboarding-card" onSubmit={finish}>
        <h1>Welcome to Coda</h1>
        <p className="secondary">
          Play music from a folder on this computer, a Navidrome server, or both. You can change
          this later in Settings.
        </p>

        <section className="onboarding-step">
          <h2>Music folder</h2>
          <div className="settings-row">
            <div className={folder ? "mono" : "secondary"}>{folder ?? "No folder chosen"}</div>
            <div className="settings-actions">
              {folder && (
                <button type="button" className="button" onClick={() => setFolder(null)} disabled={busy}>
                  Clear
                </button>
              )}
              <button type="button" className="button" onClick={choose} disabled={busy} autoFocus>
                {folder ? "Change…" : "Choose…"}
              </button>
            </div>
          </div>
        </section>

        <section className="onboarding-step">
          <label className="settings-check">
            <input
              type="checkbox"
              checked={connect}
              onChange={(e) => setConnect(e.target.checked)}
              disabled={busy || serverBlocked}
            />
            <span>Connect a Navidrome server</span>
          </label>
          {serverBlocked && settings && (
            <p className="secondary">
              {settings.serverBlockedBy.join(" and ")} set in the environment; unset to connect here.
            </p>
          )}
          {useServer && <ServerFields value={server} onChange={setServer} disabled={busy} />}
        </section>

        {error && (
          <p role="alert" className="settings-status settings-status-error">
            {error}
          </p>
        )}
        <div className="onboarding-actions">
          <button type="submit" className="button button-primary" disabled={!ready || busy}>
            {busy ? "Setting up…" : "Continue"}
          </button>
        </div>
      </form>
    </main>
  );
}
