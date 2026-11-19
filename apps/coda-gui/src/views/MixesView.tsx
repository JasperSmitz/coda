// A source's saved mixes (the shared mixes file, M12) as cards: the name,
// the ingredients as chips, Play, and a menu (right-click, "…" or the
// context-menu key) with Play, Edit… (F2) and Delete… (Delete). "New
// mix…" opens the editor. The card of the mix that plays says so.

import { useState, type KeyboardEvent } from "react";
import { ContextMenu, type MenuState } from "../components/ContextMenu";
import { Empty, SkeletonRows } from "../components/LoadState";
import { IngredientChip } from "../components/MixParts";
import { AlertIcon, MoreIcon, PlayIcon, PlusIcon } from "../components/icons";
import { usePageScroller, useScroll } from "../hooks/useLayout";
import { useLoad } from "../hooks/useLoad";
import { isRemoveKey } from "../lib/keys";
import {
  deleteMixFlow,
  editMixFlow,
  mixesApi,
  newMixFlow,
  playSavedMix,
  toMixError,
  useMixesVersion,
} from "../lib/mixes";
import { ingredientKey } from "../lib/mixRecipe";
import type { ActiveMix, Mix, SourceKind } from "../lib/types";
import type { Entry } from "../state/nav";
import { useAppState } from "../state/store";
import { ListHeader, SOURCE_LABEL, plural, useCanPlay } from "./common";

/** Chips shown on a card before "+N more". */
const CARD_CHIPS = 8;

/** A mix's `useLoad` result rejects with a `MixError`, not a `BrowseError`: keep its message. */
async function listMixes(source: SourceKind): Promise<Mix[]> {
  try {
    return await mixesApi.list(source);
  } catch (error) {
    const e = toMixError(error);
    throw { kind: e.kind === "notConfigured" ? "notConfigured" : "failed", message: e.message };
  }
}

export function MixesView({ entry, source }: { entry: Entry; source: SourceKind }) {
  const version = useMixesVersion();
  const { snapshot } = useAppState();
  const load = useLoad(() => listMixes(source), source, version);
  const scroller = usePageScroller();
  const mixes = load.data ?? [];
  useScroll(scroller, entry, mixes.length > 0);
  const [menu, setMenu] = useState<(MenuState & { mix: Mix }) | null>(null);
  const started = snapshot?.phase === "ready";
  const active = snapshot?.mix ?? null;

  return (
    <section className="view">
      <ListHeader
        eyebrow={SOURCE_LABEL[source]}
        title="Mixes"
        meta={load.state === "ready" && plural(mixes.length, "mix", "mixes")}
        tools={
          <button
            className="button"
            onClick={() => newMixFlow(source)}
            disabled={!started || load.state === "error"}
          >
            <PlusIcon size={14} /> New mix…
          </button>
        }
      />
      <div className="view-body">
        {load.state === "loading" && <SkeletonRows count={4} />}
        {load.state === "error" && (
          <div className="state-message" role="alert">
            <AlertIcon size={20} />
            <p className="state-title">
              {load.error.kind === "notConfigured" ? "Not set up" : "Could not load the mixes"}
            </p>
            <p className="secondary">{load.error.message}</p>
            {load.error.kind !== "notConfigured" && (
              <div className="state-actions">
                <button className="button" onClick={load.retry}>
                  Retry
                </button>
              </div>
            )}
          </div>
        )}
        {load.state === "ready" && mixes.length === 0 && (
          <Empty title="No mixes yet">
            A mix plays the genres, artists and albums you pick, taking turns, for as long as you
            listen.
          </Empty>
        )}
        {mixes.length > 0 && (
          <ul className="mix-cards" aria-label="Mixes">
            {mixes.map((mix) => (
              <MixCard
                key={mix.id}
                mix={mix}
                source={source}
                active={active?.id === mix.id ? active : null}
                onMenu={(x, y) => setMenu({ x, y, key: mix.id, mix })}
              />
            ))}
          </ul>
        )}
      </div>
      {menu && (
        <ContextMenu
          state={menu}
          onClose={() => setMenu(null)}
          items={[
            { label: "Play", disabled: !canPlayNow(snapshot), action: () => playSavedMix(menu.mix) },
            { label: "Edit…", disabled: !started, action: () => editMixFlow(menu.mix) },
            { label: "Delete…", disabled: !started, action: () => void deleteMixFlow(menu.mix) },
          ]}
        />
      )}
    </section>
  );
}

/** `useCanPlay` for a snapshot already at hand. */
function canPlayNow(snapshot: ReturnType<typeof useAppState>["snapshot"]): boolean {
  return snapshot?.phase === "ready" && snapshot.status !== "unavailable";
}

function MixCard({
  mix,
  source,
  active,
  onMenu,
}: {
  mix: Mix;
  source: SourceKind;
  active: ActiveMix | null;
  onMenu: (x: number, y: number) => void;
}) {
  const canPlay = useCanPlay();
  const { snapshot } = useAppState();
  const started = snapshot?.phase === "ready";
  const shown = mix.ingredients.slice(0, CARD_CHIPS);
  const more = mix.ingredients.length - shown.length;

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
      const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
      onMenu(rect.left + 24, rect.top + 40);
    } else if (e.key === "F2" && started) {
      editMixFlow(mix);
    } else if (isRemoveKey(e) && started) {
      void deleteMixFlow(mix);
    } else return;
    e.preventDefault();
    e.stopPropagation();
  };

  return (
    <li
      className={`mix-card ${active ? "is-active" : ""}`}
      aria-label={mix.name}
      onKeyDown={onKeyDown}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(e.clientX, e.clientY);
      }}
    >
      <div className="mix-card-head">
        <h3 className="mix-card-name" title={mix.name}>
          {mix.name}
        </h3>
        {active && (
          <span className="tag mix-card-state">{active.loading ? "Loading…" : "Playing"}</span>
        )}
      </div>
      <ul className="mix-chips" aria-label={`In ${mix.name}`}>
        {shown.map((ingredient, i) => (
          <IngredientChip key={`${i}:${ingredientKey(ingredient, source)}`} ingredient={ingredient} />
        ))}
        {more > 0 && <li className="mix-chip mix-chip-more">+{more} more</li>}
      </ul>
      <div className="mix-card-actions">
        <button
          className="button button-primary"
          onClick={() => playSavedMix(mix)}
          disabled={!canPlay}
          aria-label={`Play ${mix.name}`}
        >
          <PlayIcon size={13} /> Play
        </button>
        <button
          className="icon-button mix-card-more"
          aria-label={`More for ${mix.name}`}
          title="More"
          onClick={(e) => {
            const rect = e.currentTarget.getBoundingClientRect();
            onMenu(rect.left, rect.bottom);
          }}
        >
          <MoreIcon size={16} />
        </button>
      </div>
    </li>
  );
}
