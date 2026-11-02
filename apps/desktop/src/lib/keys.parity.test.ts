import { describe, expect, it } from 'vitest';
import type { Key } from '../bindings/Key';
import parity from './keys-parity.json';
import { valueAt } from './keys';

interface Fixture {
  cases: { keys: Key[]; at: number[]; values: number[] }[];
}
const fixture = parity as unknown as Fixture;

describe('keys parity with the model', () => {
  it('eases between two keys as the engine does, for every ease', () => {
    expect(fixture.cases.length).toBe(40);
    for (const c of fixture.cases) {
      const name = JSON.stringify(c.keys[1].ease);
      c.at.forEach((t, i) => expect(valueAt(c.keys, t, 0), `${name} at ${t}`).toBeCloseTo(c.values[i], 9));
    }
  });
});
