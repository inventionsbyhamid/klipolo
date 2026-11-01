import { describe, expect, it } from 'vitest';
import { boxPixels } from './boxPixels';

describe('boxPixels', () => {
  it('is_the_box_s_css_size_times_the_display_s_scale', () => {
    // The Record dialog's preview box on a 2x display, and on a 1x one.
    expect(boxPixels(672, 378, 2)).toEqual({ width: 1344, height: 756 });
    expect(boxPixels(672, 378, 1)).toEqual({ width: 672, height: 378 });
    expect(boxPixels(600.4, 337.6, 1.5)).toEqual({ width: 901, height: 506 });
  });

  it('is_null_before_the_box_has_a_size', () => {
    expect(boxPixels(0, 0, 2)).toBeNull();
    expect(boxPixels(672, 0, 2)).toBeNull();
  });

  it('reads_a_scale_it_can_t_use_as_1', () => {
    expect(boxPixels(672, 378, 0)).toEqual({ width: 672, height: 378 });
    expect(boxPixels(672, 378, Number.NaN)).toEqual({ width: 672, height: 378 });
  });
});
