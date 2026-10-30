import { afterEach, describe, expect, it, vi } from 'vitest';
import { stubSize } from './layoutBox';
import { captureResizeObservers } from './resizeObservers';

/** The shared layout helpers' own checks: the page's other tests only import them. */

const own = (proto: object, name: string) => Object.getOwnPropertyDescriptor(proto, name);
const originals = () => ({
  rect: own(Element.prototype, 'getBoundingClientRect'),
  clientWidth: own(Element.prototype, 'clientWidth'),
  clientHeight: own(Element.prototype, 'clientHeight'),
  offsetWidth: own(HTMLElement.prototype, 'offsetWidth'),
  offsetHeight: own(HTMLElement.prototype, 'offsetHeight'),
  offsetLeft: own(HTMLElement.prototype, 'offsetLeft'),
  offsetTop: own(HTMLElement.prototype, 'offsetTop'),
});
const before = originals();

function box(className: string): HTMLElement {
  const el = document.createElement('div');
  el.className = className;
  document.body.append(el);
  return el;
}
const sizeOf = (el: HTMLElement) => ({
  rect: [el.getBoundingClientRect().width, el.getBoundingClientRect().height],
  offset: [el.offsetWidth, el.offsetHeight],
  client: [el.clientWidth, el.clientHeight],
});

/** Each test's restores, run after it even when an assertion fails first, so one failure stays in its own test. */
const restores: (() => void)[] = [];
function kept<T extends () => void>(restore: T): T {
  restores.push(restore);
  return restore;
}
afterEach(() => {
  // Newest first, as nested captures need.
  for (const restore of restores.splice(0).reverse()) restore();
  document.body.replaceChildren();
});

describe('stubSize', () => {
  it('sizes matching elements live, leaves the others alone, and restores the original descriptors', () => {
    const panel = box('panel');
    const other = box('other');
    const size = { width: 440, height: 600 };
    const restore = kept(stubSize('.panel', size));
    expect(sizeOf(panel)).toEqual({ rect: [440, 600], offset: [440, 600], client: [440, 600] });
    expect(panel.getBoundingClientRect()).toMatchObject({ left: 0, top: 0, right: 440, bottom: 600 });
    expect(sizeOf(other)).toEqual({ rect: [0, 0], offset: [0, 0], client: [0, 0] });
    // The test changes the fields to resize.
    Object.assign(size, { width: 300, height: 200 });
    expect(sizeOf(panel)).toEqual({ rect: [300, 200], offset: [300, 200], client: [300, 200] });
    restore();
    expect(sizeOf(panel)).toEqual({ rect: [0, 0], offset: [0, 0], client: [0, 0] });
    expect(originals()).toEqual(before);
    expect(own(Element.prototype, 'getBoundingClientRect')!.value).toBe(before.rect!.value);
    expect(own(HTMLElement.prototype, 'offsetWidth')!.get).toBe(before.offsetWidth!.get);
  });

  it('stacks: the newest stub wins, and the originals are back once every stub is restored, in any order', () => {
    const panel = box('panel wide');
    const first = kept(stubSize('.panel', { width: 100, height: 50 }));
    const second = kept(stubSize('.wide', { width: 900, height: 70 }));
    expect(sizeOf(panel).client).toEqual([900, 70]);
    first();
    expect(sizeOf(panel).client).toEqual([900, 70]);
    second();
    second();
    expect(sizeOf(panel).client).toEqual([0, 0]);
    expect(originals()).toEqual(before);
  });

  it('places a box at its left and top, read live; without them it sits at 0, 0', () => {
    const panel = box('panel');
    const other = box('other');
    const at = { width: 100, height: 50, left: 64, top: 10 };
    kept(stubSize('.panel', at));
    kept(stubSize('.other', { width: 30, height: 20 }));
    expect(panel.getBoundingClientRect()).toMatchObject({
      x: 64,
      y: 10,
      left: 64,
      top: 10,
      right: 164,
      bottom: 60,
      width: 100,
      height: 50,
    });
    expect([panel.offsetLeft, panel.offsetTop]).toEqual([64, 10]);
    expect(other.getBoundingClientRect()).toMatchObject({ x: 0, y: 0, right: 30, bottom: 20 });
    expect([other.offsetLeft, other.offsetTop]).toEqual([0, 0]);
    Object.assign(at, { left: 200, top: 0 });
    expect(panel.getBoundingClientRect()).toMatchObject({ left: 200, right: 300, top: 0 });
    expect([panel.offsetLeft, panel.offsetTop]).toEqual([200, 0]);
  });

  it('refuses a bad selector at the call, and leaves every measurement as it was', () => {
    expect(() => kept(stubSize('.panel[', { width: 10, height: 10 }))).toThrow();
    expect(originals()).toEqual(before);
    expect(sizeOf(box('panel'))).toEqual({ rect: [0, 0], offset: [0, 0], client: [0, 0] });
  });
});

describe('captureResizeObservers', () => {
  it('fire() calls each callback with its targets’ rects, box sizes and the observer', () => {
    const shim = own(window, 'ResizeObserver');
    const observers = captureResizeObservers();
    kept(() => observers.restore());
    const panel = box('panel');
    const restoreSize = kept(stubSize('.panel', { width: 320, height: 180 }));
    const callback = vi.fn();
    const observer = new ResizeObserver(callback);
    observer.observe(panel);
    observers.fire();
    const size = { inlineSize: 320, blockSize: 180 };
    expect(callback).toHaveBeenCalledTimes(1);
    expect(callback).toHaveBeenCalledWith(
      [
        {
          target: panel,
          contentRect: expect.objectContaining({ width: 320, height: 180 }),
          borderBoxSize: [size],
          contentBoxSize: [size],
        },
      ],
      observer,
    );
    // An unobserved target, or a disconnected observer, hears nothing more.
    observer.unobserve(panel);
    observers.fire();
    observer.observe(panel);
    observer.disconnect();
    observers.fire();
    expect(callback).toHaveBeenCalledTimes(1);
    restoreSize();
    observers.restore();
    expect(own(window, 'ResizeObserver')).toEqual(shim);
  });
});
