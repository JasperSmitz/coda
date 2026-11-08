// "Customise colours": every base colour and derived token of a theme,
// grouped, each with a swatch, a hex field, the system colour picker and
// a reset. Edits preview live (the backend recomputes the derived tokens
// and contrast warnings). Saving writes a custom theme (a Ghostty file
// in the user's themes folder) and selects it; built-in and Ghostty
// themes are never changed, so editing one starts a custom copy.

import { useEffect, useRef, useState } from "react";
import { errorText, themeApi } from "../../lib/ipc";
import { customName, keepAlpha, normalizeHex, opaquePart } from "../../lib/theme";
import type { Appearance, ColorEdits, ResolvedTheme, ThemeToken } from "../../lib/types";
import { theme as themeStore } from "../../state/theme";
import { allowsAlpha, COLOR_GROUPS, colorLabel, isFileColor } from "./colorGroups";

interface Props {
  /** The theme being customised, as applied (saved accent included). */
  base: ResolvedTheme;
  /** Whether a saved accent override applies to `base`. */
  savedAccent: boolean;
  /** Every theme name (for a free default name). */
  names: string[];
  onDone: (appearance: Appearance | null) => void;
}

export function ColorEditor({ base, savedAccent, names, onDone }: Props) {
  const editingCustom = base.source === "custom";
  const [edits, setEdits] = useState<ColorEdits>({});
  const [resolved, setResolved] = useState<ResolvedTheme>(base);
  const [name, setName] = useState(() => (editingCustom ? base.name : customName(base.name, names)));
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const request = useRef(0);

  // Every edit: the backend recomputes, the result previews.
  useEffect(() => {
    const id = ++request.current;
    const timer = window.setTimeout(() => {
      themeApi
        .resolve(base.name, edits)
        .then((next) => {
          if (id !== request.current) return;
          setResolved(next);
          setError(null);
          themeStore.setPreview(next);
        })
        .catch((e: unknown) => {
          if (id === request.current) setError(errorText(e));
        });
    }, 30);
    return () => window.clearTimeout(timer);
  }, [base.name, edits]);

  // Leaving the editor ends the preview; an answer still on its way is
  // ignored (it would bring the cancelled colours back).
  useEffect(
    () => () => {
      request.current++;
      themeStore.setPreview(null);
    },
    [],
  );

  const edit = (key: string, value: string | null | undefined) =>
    setEdits((current) => {
      const next = { ...current };
      if (value === undefined) delete next[key];
      else next[key] = value;
      return next;
    });

  const save = () => {
    setSaving(true);
    themeApi
      .saveCustom(base.name, name.trim(), edits, editingCustom ? base.name : null)
      .then((appearance) => {
        themeStore.setAppearance(appearance);
        onDone(appearance);
      })
      .catch((e: unknown) => setError(errorText(e)))
      .finally(() => setSaving(false));
  };

  const tokens = new Map(resolved.tokens.map((t) => [t.key, t]));
  const changed = Object.keys(edits).length > 0;

  return (
    <div className="color-editor">
      <div className="color-editor-head">
        <label className="color-editor-name">
          <span className="secondary">Save as</span>
          <input
            type="text"
            value={name}
            maxLength={100}
            onChange={(e) => setName(e.target.value)}
            aria-label="Custom theme name"
          />
        </label>
        <div className="settings-actions">
          <button type="button" className="button" onClick={() => onDone(null)} disabled={saving}>
            Cancel
          </button>
          <button
            type="button"
            className="button button-primary"
            onClick={save}
            disabled={saving || name.trim() === "" || (editingCustom && !changed && name.trim() === base.name)}
          >
            {editingCustom ? "Save" : "Save as custom theme"}
          </button>
        </div>
      </div>
      <p className="secondary">
        {editingCustom
          ? `Editing the custom theme “${base.name}”.`
          : `“${base.name}” is not changed: saving creates a custom copy.`}{" "}
        Derived colours follow the base colours until you set them.
      </p>
      {error && <p className="settings-status settings-status-error">{error}</p>}
      {COLOR_GROUPS.map((group) => (
        <fieldset key={group.title} className="color-group">
          <legend>{group.title}</legend>
          {group.items.map((item) => {
            const token = tokens.get(item.key);
            if (!token) return null;
            const edited = item.key in edits && edits[item.key] !== null;
            // Reset: drop an unsaved edit, else the saved override.
            const resettable =
              item.key in edits ||
              (item.key === "accent" ? savedAccent || (editingCustom && token.overridden) : token.overridden && !isFileColor(item.key));
            return (
              <ColorRow
                key={item.key}
                label={item.label}
                token={token}
                edited={edited}
                computed={!token.overridden && !(item.key in edits) && !group.base}
                resettable={resettable}
                onChange={(value) => edit(item.key, value)}
                onReset={() => edit(item.key, item.key in edits ? undefined : null)}
              />
            );
          })}
        </fieldset>
      ))}
    </div>
  );
}

interface RowProps {
  label: string;
  token: ThemeToken;
  edited: boolean;
  /** Shown as computed (not set by anyone). */
  computed: boolean;
  resettable: boolean;
  onChange: (value: string) => void;
  onReset: () => void;
}

function ColorRow({ label, token, edited, computed, resettable, onChange, onReset }: RowProps) {
  const alpha = allowsAlpha(token.key);
  const [text, setText] = useState(token.value);
  const [invalid, setInvalid] = useState(false);
  useEffect(() => {
    setText(token.value);
    setInvalid(false);
  }, [token.value]);

  const commit = () => {
    const hex = normalizeHex(text, alpha);
    if (!hex) {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    if (hex !== token.value) onChange(hex);
  };

  const warning = token.contrast;
  return (
    <div className={`color-row ${edited ? "is-edited" : ""}`}>
      <span className="color-swatch" style={{ background: token.value }} aria-hidden="true" />
      <span className="color-label">
        {label}
        {computed && <span className="secondary"> · computed</span>}
        {warning && (
          <span className="color-warning" role="note">
            {" "}
            ⚠ contrast {warning.ratio.toFixed(1)}:1 against {colorLabel(warning.against).toLowerCase()}; needs{" "}
            {warning.min}:1
          </span>
        )}
      </span>
      <input
        type="text"
        className={`color-hex mono ${invalid ? "is-invalid" : ""}`}
        value={text}
        spellCheck={false}
        aria-label={`${label}, hex`}
        aria-invalid={invalid}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          } else if (e.key === "Escape") {
            e.stopPropagation();
            setText(token.value);
            setInvalid(false);
          }
        }}
      />
      <input
        type="color"
        className="color-picker"
        value={opaquePart(token.value)}
        aria-label={`${label}, colour picker`}
        onChange={(e) => onChange(keepAlpha(e.target.value, token.value))}
      />
      <button type="button" className="button button-small" onClick={onReset} disabled={!resettable}>
        Reset
      </button>
    </div>
  );
}
