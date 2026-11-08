// Loading skeletons, empty states and error states with retry.

import type { ReactNode } from "react";
import type { BrowseError } from "../lib/types";
import { nav } from "../state/nav";
import { AlertIcon } from "./icons";

export function SkeletonRows({ count = 12 }: { count?: number }) {
  return (
    <div className="skeleton-rows" aria-busy="true" aria-label="Loading">
      {Array.from({ length: count }, (_, i) => (
        <div key={i} className="skeleton-row">
          <span className="skeleton skeleton-index" />
          <span className="skeleton skeleton-text" style={{ width: `${30 + ((i * 37) % 40)}%` }} />
          <span className="skeleton skeleton-time" />
        </div>
      ))}
    </div>
  );
}

export function SkeletonGrid({ count = 12 }: { count?: number }) {
  return (
    <div className="skeleton-grid" aria-busy="true" aria-label="Loading">
      {Array.from({ length: count }, (_, i) => (
        <div key={i} className="skeleton-tile">
          <span className="skeleton skeleton-cover" />
          <span className="skeleton skeleton-text" style={{ width: "80%" }} />
          <span className="skeleton skeleton-text short" style={{ width: "50%" }} />
        </div>
      ))}
    </div>
  );
}

export function Empty({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="state-message">
      <p className="state-title">{title}</p>
      {children && <p className="secondary">{children}</p>}
    </div>
  );
}

export function ErrorState({ error, onRetry }: { error: BrowseError; onRetry: () => void }) {
  const stale = error.kind === "stale";
  return (
    <div className="state-message" role="alert">
      <AlertIcon size={20} />
      <p className="state-title">
        {error.kind === "notConfigured"
          ? "Not set up"
          : stale
            ? "This view is out of date"
            : "Could not load"}
      </p>
      <p className="secondary">{error.message}</p>
      <div className="state-actions">
        {error.kind !== "notConfigured" && (
          <button className="button" onClick={onRetry}>
            Retry
          </button>
        )}
        {(stale || error.kind === "notFound") && (
          <button className="button" onClick={() => nav.back()}>
            Go back
          </button>
        )}
      </div>
    </div>
  );
}
