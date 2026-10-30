/** Human-readable text for anything a promise rejected with or a function threw, for the person to see (a toast). */
export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
