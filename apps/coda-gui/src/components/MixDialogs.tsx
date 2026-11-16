// The mix dialogs (DESIGN.md: Mixes): the editor (a name, one search for
// genres, artists and albums of the mix's source, the chosen ones as
// removable chips, Play as a quick mix and Save) and the "Add to mix…"
// picker. Drawn by `MixDialogHost` from the dialog store (state/dialogs.ts).

import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { intent } from "../lib/ipc";
import { addToSavedMix, mixesApi, playSavedMix, saveMix, toMixError } from "../lib/mixes";
import {
  KIND_LABEL,
  MAX_INGREDIENTS,
  defaultMixName,
  describeIngredient,
  hasIngredient,
  ingredientDetail,
  ingredientKey,
  ingredientLabel,
  mergeSearchResults,
  rereadOutcome,
  sameRecipe,
  withIngredient,
} from "../lib/mixRecipe";
import { MixSearch, resultsAreCurrent, type SearchState } from "../lib/mixSearch";
import type { Ingredient, Mix, SourceKind } from "../lib/types";
import {
  answerMix,
  useDialog,
  type MixChoice,
  type MixEditorResult,
  type MixEditorSpec,
} from "../state/dialogs";
import { showUiNotice } from "../state/uiNotices";
import { useCanPlay } from "../views/common";
import { Modal } from "./Dialog";
import { PlayIcon } from "./icons";
import { IngredientChip, KindIcon } from "./MixParts";

const SOURCE_NAME: Record<SourceKind, string> = { local: "the local library", navidrome: "Navidrome" };

/** The editor's search (lib/mixSearch.ts) for `query` of `source`. */
function useMixSearch(source: SourceKind, query: string): SearchState {
  const [result, setResult] = useState<SearchState>({ state: "idle" });
  const search = useRef<MixSearch | null>(null);
  useEffect(() => {
    const s = new MixSearch(
      { genres: mixesApi.searchGenres, artists: mixesApi.searchArtists, albums: mixesApi.searchAlbums },
      setResult,
    );
    search.current = s;
    return () => {
      s.dispose();
      search.current = null;
    };
  }, []);
  useEffect(() => {
    search.current?.set(source, query);
  }, [source, query]);
  return result;
}

function MixEditor({
  spec,
  onAnswer,
}: {
  spec: MixEditorSpec;
  onAnswer: (result: MixEditorResult) => void;
}) {
  const { source } = spec;
  // The saved mix, read again from the file when the editor opens (the
  // list it was opened from may be old); its Save still replaces the
  // whole recipe (last writer wins). Save and Play wait for that re-read;
  // if it fails, the editor keeps the list's copy and says so.
  const [editing, setEditing] = useState<Mix | undefined>(spec.mix);
  const [name, setName] = useState(editing?.name ?? "");
  const [recipe, setRecipe] = useState<Ingredient[]>(
    () => editing?.ingredients ?? spec.ingredients ?? [],
  );
  const [query, setQuery] = useState("");
  const [at, setAt] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  /** For the re-read's answer, which can come while a save is in flight. */
  const savingNow = useRef(false);
  /** Whether the opening re-read has answered (a new mix has none). */
  const [settled, setSettled] = useState(spec.mix === undefined);
  const [asListed, setAsListed] = useState(false);
  /** Bumped to move the focus to the search (else a control that can take it) after a render. */
  const [focusSearch, setFocusSearch] = useState(0);
  const touched = useRef(false);
  const canPlay = useCanPlay();
  const search = useMixSearch(source, query);
  const formRef = useRef<HTMLFormElement>(null);
  const nameInput = useRef<HTMLInputElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const nameId = useId();
  const searchId = useId();
  const listId = useId();
  const noteId = useId();

  const savedId = spec.mix?.id;
  useEffect(() => {
    if (savedId === undefined) return;
    let current = true;
    mixesApi.list(source).then(
      (mixes) => {
        if (!current) return;
        setSettled(true);
        const reread = rereadOutcome(savedId, mixes, {
          saving: savingNow.current,
          touched: touched.current,
        });
        if (reread.kind === "ignore") return;
        if (reread.kind === "gone") {
          setError("That mix no longer exists");
          return;
        }
        setEditing(reread.mix);
        // Shown as saved now, unless you already started changing it.
        if (reread.show) {
          setName(reread.mix.name);
          setRecipe(reread.mix.ingredients);
        }
      },
      () => {
        if (!current) return;
        // Kept as listed; Save reports what is wrong.
        setSettled(true);
        setAsListed(true);
      },
    );
    return () => {
      current = false;
    };
  }, [source, savedId]);

  const groups = search.state === "idle" ? null : search.groups;
  const results = groups ? mergeSearchResults(groups, query, recipe, source) : [];
  // Only the completed search for what is typed now can be chosen.
  const ready = resultsAreCurrent(search, source, query);
  const busy = search.state !== "idle" && !ready;
  const index = Math.min(at, Math.max(0, results.length - 1));
  const full = recipe.length >= MAX_INGREDIENTS;
  const empty = recipe.length === 0;
  const finalName = name.trim() || defaultMixName(recipe);
  const unchanged =
    editing !== undefined && finalName === editing.name && sameRecipe(recipe, editing.ingredients);

  // The focus stays inside the dialog: after an add or a removal it goes
  // to the search (read-only at 50, never disabled, so it keeps it; never
  // to Save or the name, where a repeated Enter would save). And whenever
  // a later render removed what had it (a chip's remove button: the focus
  // falls to the page) or disabled a control of the form, it moves the
  // same way, so the global shortcuts never see the editor's keys. The
  // first render is Modal's: it focuses the name and remembers where to
  // return the focus.
  const refocus = () => searchInput.current?.focus();
  useLayoutEffect(() => {
    if (focusSearch !== 0) refocus();
  }, [focusSearch]);
  const mounted = useRef(false);
  useLayoutEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      return;
    }
    const active = document.activeElement;
    const lost =
      active === null ||
      active === document.body ||
      (formRef.current?.contains(active) === true && (active as HTMLInputElement).disabled === true);
    if (lost) refocus();
  });

  const edit = () => {
    touched.current = true;
    setError(null);
  };
  const add = (ingredient: Ingredient) => {
    if (full || saving) return;
    edit();
    setRecipe((r) => withIngredient(r, ingredient, source));
    setQuery("");
    setAt(0);
    setFocusSearch((n) => n + 1);
  };
  /** By position: a file can hold two that look the same. */
  const remove = (position: number) => {
    if (saving) return;
    edit();
    setRecipe((r) => r.filter((_, i) => i !== position));
    setFocusSearch((n) => n + 1);
  };

  const save = async () => {
    if (empty || saving || !settled) return;
    savingNow.current = true;
    setSaving(true);
    setError(null);
    try {
      const saved = await saveMix(editing?.id ?? null, {
        name: finalName,
        source,
        ingredients: recipe,
      });
      showUiNotice(`${editing ? "Saved" : "Created"} mix ${saved.name}`);
      onAnswer(saved);
    } catch (e) {
      setError(toMixError(e).message);
      savingNow.current = false;
      setSaving(false);
    }
  };

  /** A saved mix unchanged plays as itself; anything else as a quick mix, unsaved. */
  const play = () => {
    if (empty || !canPlay || saving || !settled) return;
    if (editing && unchanged) playSavedMix(editing);
    else intent({ type: "playQuickMix", mix: { name: finalName, source, ingredients: recipe } });
    onAnswer("played");
  };

  const onSearchKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    switch (e.key) {
      case "ArrowDown":
      case "ArrowUp": {
        if (results.length === 0 || !ready) return;
        const next =
          e.key === "ArrowDown"
            ? Math.min(results.length - 1, index + 1)
            : Math.max(0, index - 1);
        setAt(next);
        document.getElementById(`${listId}-${next}`)?.scrollIntoView({ block: "nearest" });
        break;
      }
      case "Enter":
        // Never submits the form (Save) from the search field, and never
        // adds a result of an earlier query.
        if (ready && results[index]) add(results[index]);
        break;
      case "Escape":
        // A first Esc clears the search; the next one closes the editor.
        if (query === "") return;
        setQuery("");
        setAt(0);
        e.stopPropagation();
        break;
      case "Backspace":
        if (query !== "" || empty || saving) return;
        remove(recipe.length - 1);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const status =
    search.state === "idle"
      ? null
      : busy
        ? "Searching…"
        : search.state === "done" && search.failed
          ? search.failed
          : search.state === "done" && results.length === 0
            ? `Nothing found for "${search.query}"`
            : null;

  // Not while saving: the save would still land after the editor closed.
  const cancel = () => {
    if (!saving) onAnswer(null);
  };

  return (
    <Modal
      title={editing ? `Edit ${editing.name}` : "New mix"}
      onCancel={cancel}
      className="dialog-mix"
    >
      <form
        ref={formRef}
        className="mix-editor"
        aria-busy={saving}
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <label className="dialog-label" htmlFor={nameId}>
          Name
        </label>
        <input
          id={nameId}
          ref={nameInput}
          className="dialog-input"
          type="text"
          value={name}
          maxLength={256}
          placeholder={defaultMixName(recipe)}
          spellCheck={false}
          autoComplete="off"
          // Read-only, not disabled, while saving: it keeps the focus.
          readOnly={saving}
          data-autofocus
          onChange={(e) => {
            edit();
            setName(e.target.value);
          }}
        />

        <label className="dialog-label mix-editor-label" htmlFor={searchId}>
          Genres, artists and albums
          <span className="secondary">
            {recipe.length} of {MAX_INGREDIENTS}
          </span>
        </label>
        {recipe.length > 0 && (
          <ul className="mix-chips mix-editor-chips" aria-label="In this mix">
            {recipe.map((ingredient, i) => (
              <IngredientChip
                key={`${i}:${ingredientKey(ingredient, source)}`}
                ingredient={ingredient}
                onRemove={() => remove(i)}
              />
            ))}
          </ul>
        )}
        <input
          id={searchId}
          ref={searchInput}
          className="dialog-input"
          type="text"
          role="combobox"
          aria-expanded={results.length > 0}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={results.length > 0 && ready ? `${listId}-${index}` : undefined}
          aria-describedby={noteId}
          value={query}
          maxLength={256}
          // Read-only, not disabled, when full: it keeps the focus.
          readOnly={saving || full}
          aria-disabled={full || undefined}
          placeholder={full ? "This mix is full" : `Search ${SOURCE_NAME[source]}`}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => {
            setQuery(e.target.value);
            setAt(0);
          }}
          onKeyDown={onSearchKeyDown}
        />
        <div
          className={`picker-list mix-results ${busy ? "is-busy" : ""}`}
          id={listId}
          role="listbox"
          aria-label="Search results"
          aria-busy={busy}
        >
          {results.map((ingredient, i) => {
            const detail = ingredientDetail(ingredient);
            const selected = ready && index === i;
            return (
              <div
                key={ingredientKey(ingredient, source)}
                id={`${listId}-${i}`}
                role="option"
                aria-selected={selected}
                aria-disabled={!ready || saving || undefined}
                aria-label={`Add ${describeIngredient(ingredient)}`}
                className={`picker-row mix-result ${selected ? "is-selected" : ""}`}
                onPointerDown={(e) => e.preventDefault() /* keeps the focus in the search */}
                onClick={() => ready && add(ingredient)}
                onPointerMove={() => ready && setAt(i)}
              >
                <KindIcon kind={ingredient.kind} size={15} />
                <span className="picker-name">
                  {ingredientLabel(ingredient)}
                  {detail && <span className="secondary"> · {detail}</span>}
                </span>
                <span className="secondary">{KIND_LABEL[ingredient.kind]}</span>
              </div>
            );
          })}
          {status && (
            <div className="picker-status secondary" role="status">
              {status}
            </div>
          )}
          {search.state === "idle" && (
            <div className="picker-status secondary">
              {empty
                ? "Type to find genres, artists and albums. They take turns when the mix plays."
                : "Type to add more."}
            </div>
          )}
        </div>
        <p className="dialog-note secondary" id={noteId}>
          {asListed
            ? "Could not read this mix again, so it is shown as the list had it."
            : editing
            ? "Changes apply the next time this mix plays."
            : "↑/↓ choose, Enter adds; Backspace in an empty search removes the last one."}
        </p>
        {error && (
          <p className="mix-editor-error" role="alert">
            {error}
          </p>
        )}
        <div className="dialog-actions">
          <button
            type="button"
            className="button"
            aria-disabled={saving || undefined}
            onClick={cancel}
          >
            Cancel
          </button>
          <button
            type="button"
            className="button"
            onClick={play}
            disabled={empty || !canPlay || saving}
            aria-disabled={!settled || undefined}
            title={
              !settled
                ? "Reading the mix…"
                : editing && unchanged
                ? `Play ${editing.name}`
                : "Play these without saving (a quick mix)"
            }
          >
            <PlayIcon size={13} /> {editing && !unchanged ? "Play changes" : "Play"}
          </button>
          <button
            type="submit"
            className="button button-primary"
            aria-disabled={saving || !settled || undefined}
            disabled={empty}
            title={settled ? undefined : "Reading the mix…"}
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

type PickerLoad =
  | { state: "loading" }
  | { state: "error"; message: string }
  | { state: "ready"; mixes: Mix[] };

/** "Add to mix…": the source's saved mixes, filtered as you type; "New mix with this…" first. */
function MixPicker({
  source,
  ingredient,
  onAnswer,
}: {
  source: SourceKind;
  ingredient: Ingredient;
  onAnswer: (choice: MixChoice) => void;
}) {
  const [load, setLoad] = useState<PickerLoad>({ state: "loading" });
  const [filter, setFilter] = useState("");
  const [at, setAt] = useState(0);
  const listId = useId();

  useEffect(() => {
    let current = true;
    mixesApi.list(source).then(
      (mixes) => current && setLoad({ state: "ready", mixes }),
      (error: unknown) => current && setLoad({ state: "error", message: toMixError(error).message }),
    );
    return () => {
      current = false;
    };
  }, [source]);

  const needle = filter.trim().toLowerCase();
  const shown =
    load.state === "ready"
      ? load.mixes.filter((m) => needle === "" || m.name.toLowerCase().includes(needle))
      : [];
  // Row 0 is "New mix with this…".
  const rows = shown.length + 1;
  const index = Math.min(at, rows - 1);
  /** Why mix `mix` might not take the ingredient (as listed): a hint only. */
  const refusal = (mix: Mix): string | null =>
    hasIngredient(mix.ingredients, ingredient, source)
      ? "Already in it"
      : mix.ingredients.length >= MAX_INGREDIENTS
        ? "Full"
        : null;
  const [refused, setRefused] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  // Any mix can be picked: the add is checked against the mix as saved
  // now (another app may have changed it since this list was read). If it
  // already holds it or is full, the picker stays open and says so.
  const pick = async (i: number) => {
    if (adding) return;
    if (i === 0) return onAnswer("new");
    const mix = shown[i - 1];
    setAdding(true);
    setRefused(`Adding to ${mix.name}…`);
    try {
      onAnswer(await addToSavedMix(mix.id, source, ingredient));
    } catch (error) {
      const e = toMixError(error);
      setRefused(
        e.kind === "mixFull"
          ? `${mix.name} is full (at most ${MAX_INGREDIENTS} genres, artists and albums)`
          : e.kind === "alreadyIn"
            ? `${ingredientLabel(ingredient)} is already in ${mix.name}`
            : `Could not add to ${mix.name}: ${e.message}`,
      );
      setAdding(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    let to: number;
    if (e.key === "ArrowDown") to = index + 1;
    else if (e.key === "ArrowUp") to = index - 1;
    else if (e.key === "Enter") {
      e.preventDefault();
      void pick(index);
      return;
    } else return;
    e.preventDefault();
    const next = Math.min(rows - 1, Math.max(0, to));
    setAt(next);
    document.getElementById(`${listId}-${next}`)?.scrollIntoView({ block: "nearest" });
  };

  return (
    <Modal
      title={`Add ${ingredientLabel(ingredient)} to a mix`}
      onCancel={() => onAnswer(null)}
      className="dialog-picker"
    >
      <input
        className="dialog-input"
        type="text"
        role="combobox"
        aria-expanded="true"
        aria-autocomplete="list"
        placeholder="Filter mixes"
        aria-label="Filter mixes"
        aria-controls={listId}
        aria-activedescendant={`${listId}-${index}`}
        value={filter}
        spellCheck={false}
        autoComplete="off"
        data-autofocus
        onChange={(e) => {
          setFilter(e.target.value);
          setAt(0);
          if (!adding) setRefused(null);
        }}
        onKeyDown={onKeyDown}
      />
      <div className="picker-list" id={listId} role="listbox" aria-label="Mixes">
        <div
          id={`${listId}-0`}
          role="option"
          aria-selected={index === 0}
          className={`picker-row picker-new ${index === 0 ? "is-selected" : ""}`}
          onClick={() => void pick(0)}
          onPointerMove={() => setAt(0)}
        >
          New mix with this…
        </div>
        {shown.map((mix, i) => {
          const why = refusal(mix);
          return (
            <div
              key={mix.id}
              id={`${listId}-${i + 1}`}
              role="option"
              aria-selected={index === i + 1}
              aria-label={why ? `${mix.name}, ${why.toLowerCase()}` : undefined}
              className={`picker-row ${index === i + 1 ? "is-selected" : ""} ${why ? "is-refused" : ""}`}
              onClick={() => void pick(i + 1)}
              onPointerMove={() => setAt(i + 1)}
            >
              <span className="picker-name">{mix.name}</span>
              <span className="secondary">{why ?? mix.ingredients.length.toLocaleString()}</span>
            </div>
          );
        })}
        {load.state === "loading" && <div className="picker-status secondary">Loading mixes…</div>}
        {load.state === "error" && (
          <div className="picker-status secondary" role="alert">
            {load.message}
          </div>
        )}
        {load.state === "ready" && load.mixes.length === 0 && (
          <div className="picker-status secondary">You have no saved mixes yet.</div>
        )}
      </div>
      <p className="dialog-note secondary" role="status">
        {refused ?? ""}
      </p>
      <div className="dialog-actions">
        <button className="button" onClick={() => onAnswer(null)}>
          Cancel
        </button>
      </div>
    </Modal>
  );
}

/** Draws the open mix dialog, if the dialog store holds one. */
export function MixDialogHost() {
  const dialog = useDialog();
  if (!dialog) return null;
  const { id } = dialog;
  switch (dialog.kind) {
    case "mixEditor":
      return <MixEditor key={id} spec={dialog.spec} onAnswer={(r) => answerMix(id, r)} />;
    case "pickMix":
      return (
        <MixPicker
          key={id}
          source={dialog.source}
          ingredient={dialog.ingredient}
          onAnswer={(c) => answerMix(id, c)}
        />
      );
    default:
      return null;
  }
}
