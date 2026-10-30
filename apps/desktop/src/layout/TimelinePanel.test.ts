import { describe, expect, it } from 'vitest';
import { ruler } from './TimelinePanel';

describe('ruler', () => {
  it('labels an empty timeline every 5 s over 40 s', () => {
    const r = ruler(0);
    expect(r.labels.map((l) => l.text)).toEqual(['0:00', '0:05', '0:10', '0:15', '0:20', '0:25', '0:30', '0:35']);
    expect(r.major).toBe('12.5%');
    expect(r.minor).toBe('2.5%');
  });

  it('picks a step that gives at most 8 labels', () => {
    const r = ruler(155_000_000);
    expect(r.labels.map((l) => l.text)).toEqual(['0:00', '0:30', '1:00', '1:30', '2:00', '2:30']);
  });
});
