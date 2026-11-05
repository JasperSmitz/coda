import type { Problem } from "../lib/types";
import { AlertIcon } from "./icons";

/** Persistent problems (no config, mpv missing, Navidrome unreachable). */
export function Banner({ problems }: { problems: Problem[] }) {
  if (problems.length === 0) return null;
  return (
    <div className="banners" role="alert">
      {problems.map((p) => (
        <div key={`${p.kind}:${p.message}`} className={`banner banner-${p.kind}`}>
          <AlertIcon size={16} />
          <span>{p.message}</span>
        </div>
      ))}
    </div>
  );
}
