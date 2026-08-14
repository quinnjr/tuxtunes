/** Extract a human-readable message from a caught value of unknown shape. */
export function toErrorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
