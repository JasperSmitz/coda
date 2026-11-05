import { intent } from "../lib/ipc";
import type { Notice } from "../lib/types";
import { CloseIcon } from "./icons";

/** Notices bottom-right above the bar; the backend expires them. */
export function Toasts({ notices }: { notices: Notice[] }) {
  return (
    <div className="toasts" aria-live="polite">
      {notices.map((n) => (
        <div key={n.id} className="toast">
          <span>{n.text}</span>
          <button
            className="icon-button"
            aria-label="Dismiss"
            onClick={() => intent({ type: "dismissNotice", id: n.id })}
          >
            <CloseIcon size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
