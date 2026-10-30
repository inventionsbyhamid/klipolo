import { render } from '@testing-library/react';
import { act } from 'react';
import { beforeEach, describe, expect, it } from 'vitest';
import { holdClock } from '../../state/playClock';
import { usePlayback } from '../../state/playback';
import { Timecode } from './Timecode';

beforeEach(() => {
  holdClock(0);
  usePlayback.setState({ playheadUs: 0, playing: false, seq: 0 });
});

describe('Timecode', () => {
  it('follows the paused playhead, never past the end of a video that got shorter', () => {
    const { container, rerender } = render(<Timecode durationUs={30_000_000} />);
    act(() => usePlayback.setState({ playheadUs: 12_345_678 }));
    expect(container.textContent).toBe('0:12.3');
    act(() => usePlayback.setState({ playheadUs: 29_900_000 }));
    // An undo took the last 20 s away: the time shows the new end, not where the playhead was.
    rerender(<Timecode durationUs={10_000_000} />);
    expect(container.textContent).toBe('0:10.0');
    rerender(<Timecode durationUs={0} />);
    expect(container.textContent).toBe('0:00.0');
  });
});
