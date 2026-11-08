import { intent } from "../lib/ipc";
import type { Notice } from "../lib/types";
import { dismissUiNotice, useUiNotices } from "../state/uiNotices";
import { CloseIcon } from "./icons";

/**
 * Notices bottom-right above the bar; the backend expires them. The UI's
 * own notices (negative ids, `state/uiNotices.ts`) show alongside.
 */
export function Toasts({ notices }: { notices: Notice[] }) {
  const own = useUiNotices();
  return (
    <div className="toasts" aria-live="polite">
      {[...notices, ...own].map((n) => (
        <div key={n.id} className="toast">
          <span>{n.text}</span>
          <button
            className="icon-button"
            aria-label="Dismiss"
            onClick={() =>
              n.id < 0 ? dismissUiNotice(n.id) : intent({ type: "dismissNotice", id: n.id })
            }
          >
            <CloseIcon size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
