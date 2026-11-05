// Shared key rules. Plain Backspace is the global Back key (DESIGN.md),
// so it never removes anything.

/**
 * The remove key in lists: Delete (also Ctrl+Delete, and fn+⌫ on Mac
 * keyboards) or Cmd+Backspace (macOS). Never plain Backspace.
 */
export function isRemoveKey(e: { key: string; metaKey: boolean; altKey: boolean }): boolean {
  if (e.altKey) return false;
  return e.key === "Delete" || (e.key === "Backspace" && e.metaKey);
}
