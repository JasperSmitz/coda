// The Navidrome server form fields with "Test connection" (a ping with
// the typed credentials; nothing is saved). Controlled by the parent,
// which owns the submit buttons. The password stays in this form's state
// and goes to the backend only on Test or Save.

import { useId, useState } from "react";
import { errorText, settingsApi } from "../lib/ipc";
import type { ServerInput } from "../lib/types";

interface Props {
  value: ServerInput;
  onChange: (value: ServerInput) => void;
  disabled?: boolean;
  autoFocus?: boolean;
}

type TestState = { kind: "idle" } | { kind: "testing" } | { kind: "ok" } | { kind: "failed"; message: string };

export function serverComplete(server: ServerInput): boolean {
  return server.url.trim() !== "" && server.username.trim() !== "" && server.password !== "";
}

export function ServerFields({ value, onChange, disabled = false, autoFocus = false }: Props) {
  const id = useId();
  const [test, setTest] = useState<TestState>({ kind: "idle" });
  const set = (patch: Partial<ServerInput>) => {
    setTest({ kind: "idle" });
    onChange({ ...value, ...patch });
  };
  const runTest = () => {
    setTest({ kind: "testing" });
    settingsApi
      .testServer(value)
      .then(() => setTest({ kind: "ok" }))
      .catch((e: unknown) => setTest({ kind: "failed", message: errorText(e) }));
  };
  return (
    <div className="server-fields">
      <label htmlFor={`${id}-url`}>Server URL</label>
      <input
        id={`${id}-url`}
        type="url"
        placeholder="https://music.example.com"
        value={value.url}
        onChange={(e) => set({ url: e.target.value })}
        disabled={disabled}
        autoFocus={autoFocus}
        autoComplete="url"
        spellCheck={false}
      />
      <label htmlFor={`${id}-user`}>Username</label>
      <input
        id={`${id}-user`}
        type="text"
        value={value.username}
        onChange={(e) => set({ username: e.target.value })}
        disabled={disabled}
        autoComplete="username"
        spellCheck={false}
      />
      <label htmlFor={`${id}-password`}>Password</label>
      <input
        id={`${id}-password`}
        type="password"
        value={value.password}
        onChange={(e) => set({ password: e.target.value })}
        disabled={disabled}
        autoComplete="current-password"
      />
      <div className="server-test">
        <button
          type="button"
          className="button"
          onClick={runTest}
          disabled={disabled || !serverComplete(value) || test.kind === "testing"}
        >
          {test.kind === "testing" ? "Testing…" : "Test connection"}
        </button>
        <span role="status" className={`field-status field-status-${test.kind}`}>
          {test.kind === "ok" && "Connected"}
          {test.kind === "failed" && test.message}
        </span>
      </div>
    </div>
  );
}
