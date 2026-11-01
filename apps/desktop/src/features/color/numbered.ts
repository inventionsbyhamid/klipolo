/** Swatches that would share a name are told apart by a number: "Yellow", "Yellow 2". */
export function numbered<T extends { name: string }>(options: T[], word: (name: string, n: number) => string): T[] {
  const seen = new Map<string, number>();
  return options.map((option) => {
    const n = (seen.get(option.name) ?? 0) + 1;
    seen.set(option.name, n);
    return n === 1 ? option : { ...option, name: word(option.name, n) };
  });
}
