// Small modal dialogs (DESIGN.md surfaces): a confirmation, a text prompt
// and the "Add to playlist" picker. Keyboard: Enter confirms, Esc
// cancels, Tab stays inside the dialog; focus returns to where it was.
// Keys never reach the global shortcuts while a dialog is open.

import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { browse } from "../lib/browse";
import type { Playlist } from "../lib/types";
import { answer, useDialog, type ConfirmSpec, type PromptSpec } from "../state/dialogs";

const FOCUSABLE =
  'button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex="-1"])';

interface ModalProps {
  title: string;
  onCancel: () => void;
  children: ReactNode;
  className?: string;
}

/** The frame: backdrop, focus trap, Esc. Focuses `[data-autofocus]` or the first control. */
export function Modal({ title, onCancel, children, className }: ModalProps) {
  const box = useRef<HTMLDivElement>(null);
  const titleId = useId();

  useLayoutEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const el = box.current;
    const first =
      el?.querySelector<HTMLElement>("[data-autofocus]") ?? el?.querySelector<HTMLElement>(FOCUSABLE);
    first?.focus();
    return () => {
      if (before?.isConnected) before.focus();
    };
  }, []);

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    } else if (e.key === "Tab") {
      const items = Array.from(box.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
      if (items.length > 0) {
        const at = items.indexOf(document.activeElement as HTMLElement);
        const next = e.shiftKey
          ? (at <= 0 ? items.length : at) - 1
          : at < 0 || at === items.length - 1
            ? 0
            : at + 1;
        items[next].focus();
      }
      e.preventDefault();
    }
    // The global shortcuts and Back never see a dialog's keys.
    e.stopPropagation();
  };

  return (
    <div
      className="dialog-backdrop"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div
        ref={box}
        className={`dialog ${className ?? ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        // A click on the dialog's body keeps the focus inside it.
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <h2 id={titleId} className="dialog-title">
          {title}
        </h2>
        {children}
      </div>
    </div>
  );
}

function ConfirmDialog({ spec, onAnswer }: { spec: ConfirmSpec; onAnswer: (ok: boolean) => void }) {
  return (
    <Modal title={spec.title} onCancel={() => onAnswer(false)}>
      <p className="dialog-message">{spec.message}</p>
      <div className="dialog-actions">
        {/* A destructive action never takes a stray Enter: Cancel has the focus. */}
        <button className="button" data-autofocus={spec.danger ? true : undefined} onClick={() => onAnswer(false)}>
          Cancel
        </button>
        <button
          className={`button ${spec.danger ? "button-danger" : "button-primary"}`}
          data-autofocus={spec.danger ? undefined : true}
          onClick={() => onAnswer(true)}
        >
          {spec.confirmLabel}
        </button>
      </div>
    </Modal>
  );
}

function PromptDialog({ spec, onAnswer }: { spec: PromptSpec; onAnswer: (value: string | null) => void }) {
  const [value, setValue] = useState(spec.initial);
  const input = useRef<HTMLInputElement>(null);
  const fieldId = useId();
  useEffect(() => input.current?.select(), []);
  const blank = value.trim() === "";
  return (
    <Modal title={spec.title} onCancel={() => onAnswer(null)}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!blank) onAnswer(value);
        }}
      >
        <label className="dialog-label" htmlFor={fieldId}>
          {spec.label}
        </label>
        <input
          id={fieldId}
          ref={input}
          className="dialog-input"
          type="text"
          value={value}
          maxLength={256}
          spellCheck={false}
          autoComplete="off"
          data-autofocus
          onChange={(e) => setValue(e.target.value)}
        />
        {spec.note && <p className="dialog-note secondary">{spec.note}</p>}
        <div className="dialog-actions">
          <button type="button" className="button" onClick={() => onAnswer(null)}>
            Cancel
          </button>
          <button type="submit" className="button button-primary" disabled={blank}>
            {spec.confirmLabel}
          </button>
        </div>
      </form>
    </Modal>
  );
}

type PickerLoad =
  | { state: "loading" }
  | { state: "error"; message: string }
  | { state: "ready"; playlists: Playlist[] };

/** Editable Navidrome playlists, filtered as you type; "New playlist…" first. */
function PlaylistPicker({ title, onAnswer }: { title: string; onAnswer: (choice: Playlist | "new" | null) => void }) {
  const [load, setLoad] = useState<PickerLoad>({ state: "loading" });
  const [filter, setFilter] = useState("");
  const [at, setAt] = useState(0);
  const listId = useId();

  useEffect(() => {
    let current = true;
    browse.playlists("navidrome").then(
      (all) => current && setLoad({ state: "ready", playlists: all.filter((p) => p.editable) }),
      (error: { message?: string }) =>
        current && setLoad({ state: "error", message: error.message ?? "Could not load the playlists" }),
    );
    return () => {
      current = false;
    };
  }, []);

  const needle = filter.trim().toLowerCase();
  const shown =
    load.state === "ready"
      ? load.playlists.filter((p) => needle === "" || p.name.toLowerCase().includes(needle))
      : [];
  // Row 0 is "New playlist…".
  const rows = shown.length + 1;
  const index = Math.min(at, rows - 1);
  const pick = (i: number) => onAnswer(i === 0 ? "new" : shown[i - 1]);

  const onKeyDown = (e: KeyboardEvent) => {
    let to: number;
    if (e.key === "ArrowDown") to = index + 1;
    else if (e.key === "ArrowUp") to = index - 1;
    else if (e.key === "Enter") {
      e.preventDefault();
      pick(index);
      return;
    } else return;
    e.preventDefault();
    const next = Math.min(rows - 1, Math.max(0, to));
    setAt(next);
    document.getElementById(`${listId}-${next}`)?.scrollIntoView({ block: "nearest" });
  };

  return (
    <Modal title={title} onCancel={() => onAnswer(null)} className="dialog-picker">
      <input
        className="dialog-input"
        type="text"
        placeholder="Filter playlists"
        aria-label="Filter playlists"
        aria-controls={listId}
        aria-activedescendant={`${listId}-${index}`}
        value={filter}
        spellCheck={false}
        autoComplete="off"
        data-autofocus
        onChange={(e) => {
          setFilter(e.target.value);
          setAt(0);
        }}
        onKeyDown={onKeyDown}
      />
      <div className="picker-list" id={listId} role="listbox" aria-label="Playlists">
        <div
          id={`${listId}-0`}
          role="option"
          aria-selected={index === 0}
          className={`picker-row picker-new ${index === 0 ? "is-selected" : ""}`}
          onClick={() => pick(0)}
          onPointerMove={() => setAt(0)}
        >
          New playlist…
        </div>
        {shown.map((p, i) => (
          <div
            key={p.id}
            id={`${listId}-${i + 1}`}
            role="option"
            aria-selected={index === i + 1}
            className={`picker-row ${index === i + 1 ? "is-selected" : ""}`}
            onClick={() => pick(i + 1)}
            onPointerMove={() => setAt(i + 1)}
          >
            <span className="picker-name">{p.name}</span>
            {p.trackCount !== null && (
              <span className="secondary">{p.trackCount.toLocaleString()}</span>
            )}
          </div>
        ))}
        {load.state === "loading" && <div className="picker-status secondary">Loading playlists…</div>}
        {load.state === "error" && <div className="picker-status secondary">{load.message}</div>}
        {load.state === "ready" && load.playlists.length === 0 && (
          <div className="picker-status secondary">You have no playlists you can change yet.</div>
        )}
      </div>
      <div className="dialog-actions">
        <button className="button" onClick={() => onAnswer(null)}>
          Cancel
        </button>
      </div>
    </Modal>
  );
}

/** Draws the open dialog (one at a time). */
export function DialogHost() {
  const dialog = useDialog();
  if (!dialog) return null;
  const { id } = dialog;
  switch (dialog.kind) {
    case "confirm":
      return <ConfirmDialog key={id} spec={dialog.spec} onAnswer={(ok) => answer(id, ok)} />;
    case "prompt":
      return <PromptDialog key={id} spec={dialog.spec} onAnswer={(v) => answer(id, v)} />;
    case "pickPlaylist":
      return <PlaylistPicker key={id} title={dialog.title} onAnswer={(c) => answer(id, c)} />;
  }
}
