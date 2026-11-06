// Settings → Appearance: how the theme is chosen (one theme, a light and
// a dark one following the system, or Ghostty's), the theme picker with
// live preview, the accent, the visualizer's colours, the colour editor
// and custom theme management. Saved to the GUI's own settings.toml.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText, themeApi } from "../../lib/ipc";
import { customName, normalizeHex, tokenValue } from "../../lib/theme";
import type { Appearance, AppearancePrefs, ResolvedTheme, Scheme, ThemeMode, ThemeSummary } from "../../lib/types";
import { confirm, promptText } from "../../state/dialogs";
import { activeTheme, systemScheme, theme as themeStore, useTheme } from "../../state/theme";
import { ColorEditor } from "./ColorEditor";
import { ThemeList } from "./ThemeList";

const MODES: { mode: ThemeMode; label: string }[] = [
  { mode: "single", label: "One theme" },
  { mode: "system", label: "Light and dark, following the system" },
  { mode: "ghostty", label: "Follow Ghostty's theme" },
];

/** The preferences' theme name for `slot` (the one the list sets). */
function slotTheme(prefs: AppearancePrefs, slot: Scheme): string {
  if (prefs.mode === "single") return prefs.theme;
  return slot === "light" ? prefs.lightTheme : prefs.darkTheme;
}

function withTheme(prefs: AppearancePrefs, slot: Scheme, name: string): AppearancePrefs {
  if (prefs.mode === "single") return { ...prefs, theme: name };
  return slot === "light" ? { ...prefs, lightTheme: name } : { ...prefs, darkTheme: name };
}

/** Previews a theme by name (cached; the newest request wins). */
function usePreview() {
  const cache = useRef(new Map<string, ResolvedTheme>());
  const latest = useRef<string | null>(null);
  const preview = useCallback((name: string | null) => {
    latest.current = name;
    if (name === null) {
      themeStore.setPreview(null);
      return;
    }
    const hit = cache.current.get(name);
    if (hit) {
      themeStore.setPreview(hit);
      return;
    }
    themeApi
      .resolve(name)
      .then((resolved) => {
        cache.current.set(name, resolved);
        if (latest.current === name) themeStore.setPreview(resolved);
      })
      .catch(() => {});
  }, []);
  const clear = useCallback(() => cache.current.clear(), []);
  // Leaving Settings ends any preview.
  useEffect(() => () => themeStore.setPreview(null), []);
  return { preview, clear };
}

export function AppearanceSection() {
  const { appearance, error: loadError } = useTheme();
  const [themes, setThemes] = useState<ThemeSummary[] | null>(null);
  const [status, setStatus] = useState<{ error: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [slot, setSlot] = useState<Scheme>(systemScheme);
  const [editing, setEditing] = useState<ResolvedTheme | null>(null);
  const [active, setActive] = useState<ThemeSummary | null>(null);
  const { preview, clear } = usePreview();

  const loadList = useCallback(() => {
    themeApi
      .list()
      .then(setThemes)
      .catch((e: unknown) => setStatus({ error: true, text: errorText(e) }));
  }, []);
  useEffect(loadList, [loadList]);

  /** Runs a change; the answer is the new state. */
  const run = useCallback(
    (work: () => Promise<Appearance>, done?: string) => {
      setBusy(true);
      themeStore.setPreview(null);
      return work()
        .then((next) => {
          themeStore.setAppearance(next);
          setStatus(done ? { error: false, text: done } : null);
          clear();
          // Names, accents (swatches) or files may have changed.
          loadList();
        })
        .catch((e: unknown) => setStatus({ error: true, text: errorText(e) }))
        .finally(() => setBusy(false));
    },
    [clear, loadList],
  );

  const names = useMemo(() => (themes ?? []).map((t) => t.name), [themes]);

  if (!appearance) {
    return (
      <section className="settings-section" aria-labelledby="settings-appearance">
        <h2 id="settings-appearance">Appearance</h2>
        <p className={loadError ? "settings-status settings-status-error" : "secondary"}>
          {loadError ?? "Loading…"}
        </p>
      </section>
    );
  }

  const { prefs } = appearance;
  const ghostty = prefs.mode === "ghostty";
  // The theme the accent and the editor act on: the one for the slot
  // being chosen (system mode), else the one showing now.
  const target: ResolvedTheme =
    prefs.mode === "system" ? (slot === "light" ? appearance.light : appearance.dark) : activeTheme(appearance);
  const setPrefs = (next: AppearancePrefs, done?: string) => void run(() => themeApi.set(next), done);

  if (editing) {
    return (
      <section className="settings-section" aria-labelledby="settings-appearance">
        <h2 id="settings-appearance">Customise colours</h2>
        <ColorEditor
          base={editing}
          savedAccent={prefs.accents[editing.name] !== undefined}
          names={names}
          onDone={(next) => {
            setEditing(null);
            if (next) {
              setStatus({ error: false, text: "Custom theme saved" });
              clear();
              loadList();
            }
          }}
        />
      </section>
    );
  }

  const activeCustom = active?.source === "custom" ? active : null;

  const renameTheme = (name: string) => {
    void promptText({ title: "Rename theme", label: "Name", initial: name, confirmLabel: "Rename" }).then((to) => {
      if (to && to.trim() !== name) void run(() => themeApi.rename(name, to.trim()), "Theme renamed");
    });
  };
  const duplicateTheme = (name: string) => {
    void promptText({
      title: "Duplicate theme",
      label: "Name of the copy",
      initial: customName(name, names),
      confirmLabel: "Duplicate",
    }).then((to) => {
      if (to) void run(() => themeApi.duplicate(name, to.trim()), "Theme duplicated");
    });
  };
  const deleteTheme = (name: string) => {
    void confirm({
      title: "Delete theme",
      message: `Delete the custom theme “${name}”? Its file is removed from the themes folder.`,
      confirmLabel: "Delete",
      danger: true,
    }).then((ok) => {
      if (ok) void run(() => themeApi.remove(name), "Theme deleted");
    });
  };

  return (
    <section className="settings-section appearance" aria-labelledby="settings-appearance">
      <h2 id="settings-appearance">Appearance</h2>

      <div className="appearance-modes" role="radiogroup" aria-label="Theme mode">
        {MODES.map(({ mode, label }) => (
          <label key={mode} className="settings-check">
            <input
              type="radio"
              name="theme-mode"
              checked={prefs.mode === mode}
              disabled={busy}
              onChange={() => setPrefs({ ...prefs, mode })}
            />
            <span>{label}</span>
          </label>
        ))}
      </div>

      {prefs.mode === "system" && (
        <div className="appearance-slots" role="tablist" aria-label="Choose the theme for">
          {(["light", "dark"] as const).map((s) => (
            <button
              key={s}
              type="button"
              role="tab"
              aria-selected={slot === s}
              className={`appearance-slot ${slot === s ? "is-current" : ""}`}
              onClick={() => setSlot(s)}
            >
              {s === "light" ? "Light" : "Dark"}: <strong>{slotTheme(prefs, s)}</strong>
            </button>
          ))}
        </div>
      )}

      {ghostty && (
        <p className="secondary">
          {appearance.ghostty
            ? appearance.ghostty.light === appearance.ghostty.dark
              ? `Ghostty uses “${appearance.ghostty.dark}”.`
              : `Ghostty uses “${appearance.ghostty.light}” (light) and “${appearance.ghostty.dark}” (dark).`
            : "Ghostty's config sets no theme."}{" "}
          Checked again whenever this window is focused.
        </p>
      )}
      {appearance.notes.map((note) => (
        <p key={note} className="settings-status settings-status-error">
          {note}
        </p>
      ))}

      {themes ? (
        <ThemeList
          themes={themes}
          selected={ghostty ? target.name : slotTheme(prefs, slot)}
          disabled={busy || ghostty}
          onPreview={preview}
          onApply={(name) => setPrefs(withTheme(prefs, slot, name))}
          onActive={setActive}
        />
      ) : (
        <p className="secondary">Loading themes…</p>
      )}

      <div className="settings-actions appearance-actions">
        <button type="button" className="button" onClick={() => setEditing(target)} disabled={busy}>
          Customise colours…
        </button>
        {active && (
          <button type="button" className="button" onClick={() => duplicateTheme(active.name)} disabled={busy}>
            Duplicate “{active.name}”…
          </button>
        )}
        {activeCustom && (
          <>
            <button type="button" className="button" onClick={() => renameTheme(activeCustom.name)} disabled={busy}>
              Rename…
            </button>
            <button type="button" className="button" onClick={() => deleteTheme(activeCustom.name)} disabled={busy}>
              Delete…
            </button>
          </>
        )}
        <button
          type="button"
          className="button"
          onClick={() => themeApi.revealFolder().catch((e: unknown) => setStatus({ error: true, text: errorText(e) }))}
          title={appearance.themesFolder ?? undefined}
        >
          Reveal themes folder
        </button>
      </div>

      <AccentPicker
        theme={target}
        saved={prefs.accents[target.name] ?? null}
        disabled={busy}
        onSet={(hex) => {
          const accents = { ...prefs.accents };
          if (hex) accents[target.name] = hex;
          else delete accents[target.name];
          setPrefs({ ...prefs, accents }, hex ? "Accent saved" : "Accent reset to the theme's");
        }}
      />

      <div className="appearance-row">
        <span>Visualizer bars</span>
        <div className="appearance-inline" role="radiogroup" aria-label="Visualizer colours">
          {(
            [
              ["accent", "Accent colour"],
              ["palette", "Palette gradient"],
            ] as const
          ).map(([value, label]) => (
            <label key={value} className="settings-check">
              <input
                type="radio"
                name="visualizer-colors"
                checked={prefs.visualizer === value}
                disabled={busy}
                onChange={() => setPrefs({ ...prefs, visualizer: value })}
              />
              <span>{label}</span>
            </label>
          ))}
        </div>
      </div>

      {status && (
        <p role="status" className={`settings-status ${status.error ? "settings-status-error" : ""}`}>
          {status.text}
        </p>
      )}
      {appearance.themesFolder && (
        <p className="settings-footnote secondary">
          Custom themes are Ghostty theme files in <span className="mono">{appearance.themesFolder}</span>; themes
          from Ghostty's folders are listed too.
        </p>
      )}
    </section>
  );
}

function AccentPicker({
  theme,
  saved,
  disabled,
  onSet,
}: {
  theme: ResolvedTheme;
  saved: string | null;
  disabled: boolean;
  onSet: (hex: string | null) => void;
}) {
  const accent = theme.tokens.find((t) => t.key === "accent");
  const [custom, setCustom] = useState("");
  const parsed = normalizeHex(custom);
  return (
    <div className="appearance-row">
      <span>
        Accent for “{theme.name}”
        {accent?.contrast && (
          <span className="color-warning" role="note">
            {" "}
            ⚠ contrast {accent.contrast.ratio.toFixed(1)}:1 on the background; needs {accent.contrast.min}:1
          </span>
        )}
      </span>
      <div className="accent-picker">
        {Array.from({ length: 16 }, (_, n) => {
          const value = tokenValue(theme, `palette-${n}`);
          const current = accent?.value === value;
          return (
            <button
              key={n}
              type="button"
              className={`accent-swatch ${current ? "is-current" : ""}`}
              style={{ background: value }}
              aria-label={`Palette colour ${n} (${value})`}
              aria-pressed={current}
              disabled={disabled}
              onClick={() => onSet(value)}
            />
          );
        })}
        <input
          type="text"
          className="color-hex mono"
          placeholder="#rrggbb"
          aria-label="Custom accent colour, hex"
          value={custom}
          onChange={(e) => setCustom(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && parsed) onSet(parsed);
          }}
        />
        <button type="button" className="button button-small" disabled={disabled || !parsed} onClick={() => onSet(parsed)}>
          Set
        </button>
        <button type="button" className="button button-small" disabled={disabled || saved === null} onClick={() => onSet(null)}>
          Reset to theme default
        </button>
      </div>
    </div>
  );
}
