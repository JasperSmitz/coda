// Small pieces of the mixes UI: an ingredient's kind icon, its chip
// (removable in the editor) and the "Mix: <name>" line that Now Playing
// and the queue show while a mix is active. (Page headers offer "Add to
// mix…" in their "…" menu: views/common.tsx `PageActions`.)

import { activeMixLabel, describeIngredient, ingredientDetail, ingredientLabel } from "../lib/mixRecipe";
import type { ActiveMix, Ingredient } from "../lib/types";
import { AlbumIcon, ArtistIcon, CloseIcon, GenreIcon, MixIcon } from "./icons";

export function KindIcon({ kind, size = 13 }: { kind: Ingredient["kind"]; size?: number }) {
  switch (kind) {
    case "genre":
      return <GenreIcon size={size} />;
    case "artist":
      return <ArtistIcon size={size} />;
    case "album":
      return <AlbumIcon size={size} />;
  }
}

/** One ingredient as a chip; with `onRemove`, a remove button too. */
export function IngredientChip({
  ingredient,
  onRemove,
}: {
  ingredient: Ingredient;
  onRemove?: () => void;
}) {
  const detail = ingredientDetail(ingredient);
  const label = ingredientLabel(ingredient);
  const description = describeIngredient(ingredient);
  return (
    <li className={`mix-chip mix-chip-${ingredient.kind}`} title={description}>
      <KindIcon kind={ingredient.kind} />
      <span className="mix-chip-text">
        <span className="visually-hidden">{ingredient.kind} </span>
        {label}
        {detail && <span className="mix-chip-detail"> · {detail}</span>}
      </span>
      {onRemove && (
        <button
          type="button"
          className="mix-chip-remove"
          aria-label={`Remove ${description}`}
          title="Remove"
          onClick={onRemove}
        >
          <CloseIcon size={12} />
        </button>
      )}
    </li>
  );
}

/** "Mix: <name>" (or "Quick mix: …"), dimmed with a pulsing dot while its songs load. */
export function ActiveMixLine({ mix, className }: { mix: ActiveMix; className?: string }) {
  const label = activeMixLabel(mix);
  return (
    <div
      className={`active-mix ${mix.loading ? "is-loading" : ""} ${className ?? ""}`}
      title={mix.loading ? `${label} (loading its songs)` : label}
    >
      <MixIcon size={14} />
      <span className="active-mix-name">{label}</span>
      {mix.loading && (
        <span className="active-mix-loading">
          <span className="scan-dot" aria-hidden="true" />
          Loading…
        </span>
      )}
    </div>
  );
}
