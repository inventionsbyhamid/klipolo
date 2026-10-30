/** Renders per block id, counted only under Vitest (`import.meta.env.MODE` is a build-time constant). */
export const renderCounts = new Map<string, number>();

export function countRender(id: string) {
  if (import.meta.env.MODE !== 'test') return;
  renderCounts.set(id, (renderCounts.get(id) ?? 0) + 1);
}
