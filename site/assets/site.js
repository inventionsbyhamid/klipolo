/* Klipolo: the words are the edit. Plain ES2020, loaded as a module (deferred, strict), no dependencies.
   Each story moment plays once, when it scrolls into view. Haptics answer only a visitor who has touched the page. */

const d = document;
d.documentElement.classList.add('js');

const reduce = matchMedia('(prefers-reduced-motion: reduce)');
let still = reduce.matches;
reduce.addEventListener?.('change', (e) => { still = e.matches; });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const make = (tag, cls, text) => {
  const e = d.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
};
const rand = (a, b) => a + Math.random() * (b - a);

// Browsers ignore vibrate() until the visitor has touched the page.
let touched = false;
for (const t of ['pointerdown', 'keydown', 'touchstart']) addEventListener(t, () => { touched = true; }, { capture: true, passive: true });

/* -- in view: a demo's typing holds while it is off-screen or the tab is hidden -- */

const seen = new WeakSet();
let holds = [];
const recheck = () => {
  const h = holds;
  holds = [];
  h.forEach((r) => r());
};
const view = 'IntersectionObserver' in window && new IntersectionObserver((es) => {
  for (const e of es) {
    if (e.isIntersecting) seen.add(e.target);
    else seen.delete(e.target);
  }
  recheck();
});
d.addEventListener('visibilitychange', recheck);
const watch = (el) => { if (view) view.observe(el); };
const live = (el) => !d.hidden && (!view || seen.has(el));
// Sleeps, then waits while `el` is out of view, the tab is hidden, or `stop()` says so.
const hold = async (el, ms, stop) => {
  await sleep(ms);
  while (!live(el) || (stop && stop())) await new Promise((r) => holds.push(r));
};

/* -- haptics -- */

const Haptic = (() => {
  const canVibrate = typeof navigator.vibrate === 'function';
  return {
    tap(ms) {
      if (still || !touched || !canVibrate) return;
      try { navigator.vibrate(ms); } catch (_) { /* blocked */ }
    },
  };
})();

/* -- typing -- */

const pause = (ch) => (ch === ' ' ? rand(70, 150) : rand(40, 95) + (Math.random() < 0.08 ? 150 : 0));

// Types a heading letter by letter. Screen readers get the whole sentence at once.
function typer(node) {
  const text = node.textContent.trim();
  const shown = make('span');
  const caret = make('span', 'caret');
  shown.setAttribute('aria-hidden', 'true');
  const chars = Array.from(text, (c) => shown.appendChild(make('span', 'ch', c)));
  node.replaceChildren(make('span', 'sr-only', text), shown);
  return {
    async run(from) {
      if (still) {
        for (const s of chars) s.classList.add('on');
        shown.append(caret);
        return;
      }
      node.classList.add('is-typing');
      shown.prepend(caret);
      for (const s of chars) {
        s.classList.add('on');
        s.after(caret);
        await hold(from, pause(s.textContent));
      }
      node.classList.remove('is-typing');
    },
  };
}

// A command line that types itself, key by key; `from` is the demo it belongs to.
async function typeInto(el, text, from, wait = (ms) => hold(from, ms)) {
  const line = el.parentElement;
  line.classList.add('is-typing');
  for (let i = 1; i <= text.length; i++) {
    el.textContent = text.slice(0, i);
    await wait(pause(text[i - 1]));
  }
  line.classList.remove('is-typing');
}

async function erase(el, wait = sleep) {
  const text = el.textContent;
  for (let i = text.length; i > 0; i -= 3) {
    el.textContent = text.slice(0, i);
    await wait(16);
  }
  el.textContent = '';
}

/* -- story moments -- */

const typers = new Map();
for (const el of d.querySelectorAll('[data-type], [data-ask]')) typers.set(el, typer(el));
const typerIn = (el, sel) => typers.get(el.querySelector(sel));

const moments = {
  async problem(el) {
    const line = el.querySelector('.cmd--answer [data-cmd]');
    const pile = el.querySelector('[data-pile]');
    if (still || !line || !pile) return;
    // One typed request clears the chores: a first glimpse of the answer.
    await hold(el, 3000);
    el.classList.add('is-asking');
    await hold(el, 300);
    await typeInto(line, 'make it polished', el);
    await hold(el, 420);
    el.classList.add('is-struck');
    await hold(el, 1100);
    const gap = parseFloat(getComputedStyle(el).rowGap) || 0;
    el.style.setProperty('--fold', `${pile.offsetHeight + gap}px`);
    el.classList.add('is-folded');
  },
  async turn(el) {
    const t = typerIn(el, '[data-type]');
    if (still) { t.run(); return; }
    await hold(el, 250);
    await t.run(el);
  },
  async say(el) {
    await typerIn(el, '[data-ask]').run(el);
    if (still) return;
    await hold(el, 450);
    el.classList.add('is-preview');
    await hold(el, 1400);
    el.classList.add('is-done');
    await hold(el, 260);
    const preview = el.querySelector('[data-preview]');
    if (preview) preview.textContent = 'Kept the 5 best moments. 0:58, vertical.';
  },
};

function fire(el) {
  el.classList.add('is-on');
  Haptic.tap(3);
  const fn = moments[el.dataset.moment];
  if (fn) fn(el);
}

const momentEls = d.querySelectorAll('[data-moment]');
if ('IntersectionObserver' in window) {
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      io.unobserve(e.target);
      fire(e.target);
    }
  }, { rootMargin: '0px 0px -30% 0px' });
  momentEls.forEach((el) => { watch(el); io.observe(el); });
} else {
  momentEls.forEach(fire);
}

/* -- hero: the words are the edit -- */

function takeDemo(take) {
  const box = take.querySelector('[data-words]');
  const timeEl = take.querySelector('[data-time]');
  const rule = take.querySelector('[data-rule]');
  const under = take.querySelector('[data-under]');
  const cmd = take.querySelector('[data-cmd]');
  const toggle = d.querySelector('[data-demo-toggle]');
  if (still || !box || !cmd) return; // the static before and after stay

  // The paragraph is laid out invisibly; a visible copy of each word is placed over it
  // with transforms, so edits reflow smoothly and the page's layout never shifts.
  take.classList.add('is-live');
  box.append(under);
  const items = Array.from(box.querySelectorAll('.w'), (m) => {
    const v = m.cloneNode(true);
    v.className = m.className.replace(/\bw\b/, 'wv');
    box.append(v);
    const c = m.classList;
    return { m, v, f: c.contains('f'), g: c.contains('g'), k: c.contains('k'), gone: false, sx: 1 };
  });

  const FULL = 72;
  let secs = FULL;
  let shown = FULL;
  let raf = 0;
  const fmt = (s) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;

  function showTime(target, now) {
    secs = target;
    const k = target / FULL;
    rule.style.transform = `scaleX(${k})`;
    timeEl.style.transform = `translateX(${-(1 - k) * rule.offsetWidth}px)`;
    cancelAnimationFrame(raf);
    if (now) { shown = target; timeEl.textContent = fmt(target); return; }
    const from = shown;
    const t0 = performance.now();
    const step = (t) => {
      const p = Math.min(1, Math.max(0, (t - t0) / 650)); // a frame can be stamped before t0
      shown = Math.round(from + (target - from) * (1 - Math.pow(1 - p, 3)));
      timeEl.textContent = fmt(shown);
      if (p < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
  }

  const pos = (it) => `translate(${it.m.offsetLeft}px, ${it.m.offsetTop}px)${it.sx === 1 ? '' : ` scaleX(${it.sx})`}`;
  // Words on the same line slide; a word that changes line fades out, moves, and fades back in.
  const place = (hop = true) => {
    for (const it of items) {
      if (it.gone) continue;
      const { v, m } = it;
      it.tf = pos(it);
      if (hop && it.y !== undefined && m.offsetTop !== it.y) {
        v.classList.add('is-hop');
        setTimeout(() => {
          v.style.transition = 'none';
          v.style.transform = it.tf;
          void v.offsetWidth;
          v.style.transition = '';
          v.classList.remove('is-hop');
        }, 130);
      } else v.style.transform = it.tf;
      it.y = m.offsetTop;
    }
  };

  function settle() {
    take.classList.add('no-tx');
    box.classList.add('is-measuring'); // every word in flow: the raw height
    box.style.minHeight = '';
    box.style.minHeight = `${box.offsetHeight}px`;
    box.classList.remove('is-measuring');
    place(false);
    showTime(secs, true);
    void box.offsetWidth;
    take.classList.remove('no-tx');
  }
  settle();
  if (window.ResizeObserver) new ResizeObserver(() => settle()).observe(box);
  if (d.fonts && d.fonts.ready) d.fonts.ready.then(settle);

  // Pausing: the Pause button, off-screen, or a hidden tab (see `hold`).
  let paused = false;
  watch(take);
  const wait = (ms) => hold(take, ms, () => paused);
  if (toggle) {
    toggle.hidden = false;
    toggle.addEventListener('click', () => {
      paused = !paused;
      toggle.textContent = paused ? 'Play the demo' : 'Pause the demo';
      recheck();
    });
  }

  const drop = (it) => { it.gone = true; it.m.classList.add('is-gone'); };
  const said = (m) => 90 + m.textContent.length * 12 + (/\.$/.test(m.textContent) ? 220 : /,$/.test(m.textContent) ? 70 : 0);

  const steps = [
    ['cut the ums', async () => {
      for (const it of items.filter((i) => i.f && !i.gone)) {
        it.v.classList.add('is-struck');
        await wait(220);
        it.v.classList.add('is-out');
        await wait(170);
        drop(it);
        place();
        showTime(secs - 1);
        await wait(420);
      }
    }],
    ['remove the long pauses', async () => {
      for (const it of items.filter((i) => i.g && !i.gone)) {
        it.sx = 0.001;
        it.v.style.transform = pos(it);
        it.v.classList.add('is-out');
        await wait(200);
        drop(it);
        place();
        showTime(secs - parseInt(it.m.textContent, 10));
        await wait(380);
      }
    }],
    ['make it 30 seconds', async () => {
      const weak = items.filter((i) => i.k && !i.gone);
      weak.forEach((it) => it.v.classList.add('is-out'));
      await wait(560);
      weak.forEach(drop);
      place();
      showTime(28);
      await wait(300);
    }],
    ['add captions', async () => {
      const words = items.filter((i) => !i.gone);
      const mark = (m) => {
        under.style.transform = `translate(${m.offsetLeft}px, ${m.offsetTop + m.offsetHeight * 0.88}px) scaleX(${m.offsetWidth / 100})`;
      };
      take.classList.add('no-tx');
      mark(words[0].m);
      void box.offsetWidth;
      take.classList.remove('no-tx');
      take.classList.add('is-reading');
      for (const it of words) {
        mark(it.m);
        it.v.classList.add('is-said');
        await wait(said(it.m));
      }
      await wait(400);
      under.style.opacity = '0';
    }],
  ];

  // Between loops the raw take fades back in, with no request typed: the demo starts over.
  async function rewind() {
    take.classList.add('is-rewinding');
    await wait(420);
    take.classList.remove('is-reading');
    under.style.opacity = '';
    const back = items.filter((it) => it.gone);
    for (const it of items) it.v.classList.remove('is-said', 'is-struck');
    for (const it of back) { it.gone = false; it.sx = 1; it.y = undefined; it.m.classList.remove('is-gone'); it.v.classList.remove('is-out'); }
    take.classList.add('no-tx');
    place(false);
    showTime(FULL, true);
    void box.offsetWidth;
    take.classList.remove('no-tx');
    take.classList.remove('is-rewinding');
    await wait(900);
  }

  (async () => {
    await wait(900);
    for (;;) {
      for (const [say, act] of steps) {
        await typeInto(cmd, say, take, wait);
        await wait(rand(380, 520));
        await act();
        await wait(1500);
        await erase(cmd, wait);
        await wait(420);
      }
      await wait(800);
      await rewind();
    }
  })();
}

const take = d.querySelector('[data-take]');
if (take) takeDemo(take);
