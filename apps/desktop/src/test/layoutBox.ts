/**
 * jsdom has no layout: every element measures 0 × 0 at 0, 0. `stubSize` gives the elements matching a selector a size,
 * and optionally a place, for code that measures with `getBoundingClientRect`, `offsetWidth/Height`,
 * `offsetLeft/Top` or `clientWidth/Height`. Shared by the UI tests; a fix to it reaches them all.
 *
 * The stub sits on `Element.prototype` (and `offset*` on `HTMLElement.prototype`). A test's own
 * `HTMLElement.prototype.clientWidth/clientHeight` therefore takes precedence over it on HTML elements (the preview,
 * timeline and captions tests define one); `getBoundingClientRect` and `offset*` still follow the stub.
 */

/** A box's size and its place (left, top: 0 when left out). */
export interface StubBox {
  width: number;
  height: number;
  left?: number;
  top?: number;
}

interface Stub {
  selector: string;
  size: StubBox;
}

/** The measurements stubbed, on the prototype that defines each in jsdom. */
const MEASURES: [object, string][] = [
  [Element.prototype, 'getBoundingClientRect'],
  [Element.prototype, 'clientWidth'],
  [Element.prototype, 'clientHeight'],
  [HTMLElement.prototype, 'offsetWidth'],
  [HTMLElement.prototype, 'offsetHeight'],
  [HTMLElement.prototype, 'offsetLeft'],
  [HTMLElement.prototype, 'offsetTop'],
];

/** The live stubs, oldest first; the newest that matches an element sizes it. */
const stubs: Stub[] = [];
/** The original descriptors while any stub is live. */
let originals: (PropertyDescriptor | undefined)[] | null = null;

function sizeOf(el: unknown): Stub['size'] | null {
  if (!(el instanceof Element)) return null;
  for (let i = stubs.length - 1; i >= 0; i--) if (el.matches(stubs[i].selector)) return stubs[i].size;
  return null;
}

function rect({ width, height, left = 0, top = 0 }: StubBox): DOMRect {
  const r = { x: left, y: top, left, top, width, height, right: left + width, bottom: top + height };
  return { ...r, toJSON: () => r };
}

function install() {
  const saved = MEASURES.map(([proto, name]) => Object.getOwnPropertyDescriptor(proto, name));
  originals = saved;
  const [bounds, clientWidth, clientHeight, offsetWidth, offsetHeight, offsetLeft, offsetTop] = saved;
  const getter = (original: PropertyDescriptor | undefined, side: keyof StubBox) =>
    function (this: Element) {
      const size = sizeOf(this);
      return size ? (size[side] ?? 0) : original?.get?.call(this);
    };
  Object.defineProperty(Element.prototype, 'getBoundingClientRect', {
    ...bounds,
    value(this: Element) {
      const size = sizeOf(this);
      return size ? rect(size) : bounds?.value.call(this);
    },
  });
  const define = (
    proto: object,
    name: string,
    original: PropertyDescriptor | undefined,
    side: keyof StubBox,
  ) =>
    Object.defineProperty(proto, name, {
      configurable: true,
      enumerable: original?.enumerable,
      get: getter(original, side),
    });
  define(Element.prototype, 'clientWidth', clientWidth, 'width');
  define(Element.prototype, 'clientHeight', clientHeight, 'height');
  define(HTMLElement.prototype, 'offsetWidth', offsetWidth, 'width');
  define(HTMLElement.prototype, 'offsetHeight', offsetHeight, 'height');
  define(HTMLElement.prototype, 'offsetLeft', offsetLeft, 'left');
  define(HTMLElement.prototype, 'offsetTop', offsetTop, 'top');
}

function uninstall() {
  MEASURES.forEach(([proto, name], i) => {
    const original = originals?.[i];
    if (original) Object.defineProperty(proto, name, original);
    else delete (proto as Record<string, unknown>)[name];
  });
  originals = null;
}

/**
 * Gives the elements matching `selector` the box `size`, read live: a test changes its fields to resize or move it. The
 * rect sits at `left`, `top` (0, 0 when left out), and `offsetLeft/Top` report them. Stubs stack (the newest match
 * wins). The returned function removes this stub; once none is left, the original descriptors are back.
 */
export function stubSize(selector: string, size: StubBox): () => void {
  // A bad selector throws here, not in every later measurement of any element.
  document.createElement('div').matches(selector);
  if (!originals) install();
  const stub = { selector, size };
  stubs.push(stub);
  return () => {
    const at = stubs.indexOf(stub);
    if (at < 0) return;
    stubs.splice(at, 1);
    if (stubs.length === 0) uninstall();
  };
}
