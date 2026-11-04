/* Klipolo: the words are the edit. Plain ES2020, loaded as a module (deferred, strict), no dependencies.
   Each story moment plays once, when it scrolls into view. Sound is off until the visitor turns it on.
   Haptics answer only a visitor who has touched the page. */

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
const store = {
  get(k) { try { return localStorage.getItem(k); } catch (_) { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch (_) { /* private mode */ } },
};

// Browsers ignore vibrate() and audio until the visitor has touched the page.
let touched = false;
for (const t of ['pointerdown', 'keydown', 'touchstart']) addEventListener(t, () => { touched = true; }, { capture: true, passive: true });

/* -- in view: a demo's own sounds and typing hold while it is off-screen or the tab is hidden -- */

const seen = new WeakSet();
let holds = [];
const recheck = () => {
  const h = holds;
  holds = [];
  h.forEach((r) => r());
  Sound.hush();
};
// A demo counts as in view while half of it shows, or it fills half the window: scrolled past, it goes quiet.
const steps = Array.from({ length: 21 }, (_, i) => i / 20);
const view = 'IntersectionObserver' in window && new IntersectionObserver((es) => {
  for (const e of es) {
    const shown = e.intersectionRect.height;
    const enough = Math.min(e.boundingClientRect.height, innerHeight) * 0.5;
    if (e.isIntersecting && shown >= enough) seen.add(e.target);
    else seen.delete(e.target);
  }
  recheck();
}, { threshold: steps });
d.addEventListener('visibilitychange', recheck);
const watch = (el) => { if (view) view.observe(el); };
const live = (el) => !d.hidden && (!view || seen.has(el));
// Sleeps, then waits while `el` is out of view, the tab is hidden, or `stop()` says so.
const hold = async (el, ms, stop) => {
  await sleep(ms);
  while (!live(el) || (stop && stop())) await new Promise((r) => holds.push(r));
};

/* -- sound -- */

const Sound = (() => {
  const KEY = 'klipolo:sound';
  // Sprite positions, [start, length] in seconds. k0 to k7 are the keystrokes.
  const MAP = {
    k0: [0.12, 0.065], k1: [0.3047, 0.065], k2: [0.4894, 0.06], k3: [0.6691, 0.052], k4: [0.8407, 0.065],
    k5: [1.0254, 0.057], k6: [1.2021, 0.064], k7: [1.3855, 0.063], thock: [1.5677, 0.147], tick: [1.8346, 0.056],
  };
  const ONSET = 0.1283; // where k0 first rises above 0.02 in the source
  const M4A = '/assets/sounds/sfx.m4a';
  const AC = window.AudioContext || window.webkitAudioContext;
  let on = store.get(KEY) === 'on';
  let ctx; let master; let buffer; let noise; let bytes; let decoding;
  let failed = false; let shift = 0; let last = -1; let voices = 0;
  const playing = new Set(); // demo voices, stopped the moment their demo leaves view

  const get = (url) => fetch(url).then((r) => { if (!r.ok) throw new Error(r.status); return r.arrayBuffer(); });
  // Fetch early (no audio context needed); decode after a gesture.
  const prefetch = () => bytes || (bytes = (d.createElement('audio').canPlayType('audio/webm; codecs="opus"')
    ? get('/assets/sounds/sfx.webm') : Promise.reject()).catch(() => get(M4A)));
  const decode = (b) => new Promise((res, rej) => { const p = ctx.decodeAudioData(b, res, rej); if (p && p.catch) p.catch(rej); });

  const load = () => decoding || (decoding = prefetch()
    .then((b) => decode(b).catch(() => get(M4A).then(decode))) // WebM said "maybe" but won't decode
    .then((buf) => {
      buffer = buf;
      // AAC can add encoder delay: find the first onset and correct for it.
      const ch = buf.getChannelData(0);
      let i = 0;
      while (i < ch.length && Math.abs(ch[i]) <= 0.02) i++;
      shift = Math.max(-0.01, Math.min(0.08, i / buf.sampleRate - ONSET));
    })
    .catch(() => { failed = true; }));

  function wake() {
    if (!on || !AC) return;
    if (!ctx) {
      try { navigator.audioSession.type = 'ambient'; } catch (_) { /* follows the iPhone's silent switch where supported */ }
      ctx = new AC();
      master = ctx.createGain();
      master.gain.value = 0.6;
      master.connect(ctx.destination);
    }
    if (ctx.state !== 'running') ctx.resume().catch(() => {});
    load();
  }

  // A refined synth for when the samples can't load.
  function synth(kind, gain, rate) {
    const c = ctx;
    const t = c.currentTime + 0.002;
    const out = c.createGain();
    out.gain.value = gain;
    out.connect(master);
    if (!noise) {
      noise = c.createBuffer(1, Math.floor(c.sampleRate * 0.08), c.sampleRate);
      const n = noise.getChannelData(0);
      for (let i = 0; i < n.length; i++) n[i] = (Math.random() * 2 - 1) * (1 - i / n.length) ** 5;
    }
    const amp = (level, dur) => {
      const g = c.createGain();
      g.gain.setValueAtTime(0.0008, t);
      g.gain.exponentialRampToValueAtTime(level, t + 0.003);
      g.gain.exponentialRampToValueAtTime(0.0008, t + dur);
      g.connect(out);
      return g;
    };
    const click = (level, freq, q, dur) => {
      const s = c.createBufferSource();
      const f = c.createBiquadFilter();
      s.buffer = noise;
      s.playbackRate.value = rate * rand(0.85, 1.15);
      f.type = 'bandpass';
      f.frequency.value = freq;
      f.Q.value = q;
      s.connect(f).connect(amp(level, dur));
      s.start(t);
    };
    const body = (level, from, to, dur) => {
      const o = c.createOscillator();
      o.frequency.setValueAtTime(from * rate, t);
      o.frequency.exponentialRampToValueAtTime(to * rate, t + dur * 0.8);
      o.connect(amp(level, dur));
      o.start(t);
      o.stop(t + dur + 0.02);
    };
    if (kind === 'thock') { body(0.7, 150, 52, 0.2); click(0.25, 900, 0.9, 0.06); }
    else if (kind === 'tick') click(0.12, 7000, 2.5, 0.025);
    else { click(0.45, rand(1900, 3000), 1.3, 0.045); body(0.1, rand(170, 220), 90, 0.05); }
  }

  // `from` is the demo a sound belongs to: it plays only while that demo is in view. The
  // visitor's own sounds pass no `from` and always play.
  function play(name, gain, rate, from) {
    if (!on || !ctx || ctx.state !== 'running' || voices > 14 || (from && !live(from))) return;
    if (!buffer) { if (failed) synth(name[0] === 'k' ? 'key' : name, gain, rate); return; }
    const [at, dur] = MAP[name];
    const s = ctx.createBufferSource();
    const g = ctx.createGain();
    s.buffer = buffer;
    s.playbackRate.value = rate;
    g.gain.value = gain;
    s.connect(g).connect(master);
    voices++;
    if (from) { s.from = from; playing.add(s); }
    s.onended = () => { voices--; playing.delete(s); };
    s.start(ctx.currentTime, Math.max(0, at + shift - 0.003), dur + 0.01);
  }

  if (on && AC) {
    // Remembered from last time: get the bytes ready, start on the first gesture.
    (window.requestIdleCallback || ((fn) => setTimeout(fn, 1200)))(prefetch);
    const first = () => { wake(); for (const t of ['pointerdown', 'keydown', 'touchend']) removeEventListener(t, first, true); };
    for (const t of ['pointerdown', 'keydown', 'touchend']) addEventListener(t, first, true);
  }

  return {
    supported: Boolean(AC),
    get on() { return on; },
    set(value) {
      on = value;
      store.set(KEY, on ? 'on' : 'off');
      if (on) wake();
      else if (ctx && ctx.state === 'running') ctx.suspend().catch(() => {});
    },
    ready() { wake(); return decoding || Promise.resolve(); },
    hush() { for (const s of playing) if (!live(s.from)) s.stop(); },
    key(soft = 1, from) {
      let i = Math.floor(Math.random() * 8);
      if (i === last) i = (i + 1 + Math.floor(Math.random() * 7)) % 8; // never the same key twice
      last = i;
      play(`k${i}`, soft * rand(0.85, 1.15), rand(0.96, 1.04), from);
    },
    thock: (soft = 1, from) => play('thock', soft * rand(0.92, 1.05), rand(0.97, 1.02), from),
    tick: (soft = 1, from) => play('tick', 0.42 * soft * rand(0.85, 1.15), rand(0.95, 1.05), from),
  };
})();

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

/* -- sound button -- */

const soundButton = d.querySelector('[data-sound]');
if (soundButton && Sound.supported) {
  const state = soundButton.querySelector('[data-sound-state]');
  const paint = () => {
    soundButton.setAttribute('aria-pressed', String(Sound.on));
    if (state) state.textContent = Sound.on ? 'on' : 'off';
  };
  paint();
  soundButton.hidden = false;
  soundButton.addEventListener('click', () => {
    Sound.set(!Sound.on);
    paint();
    if (Sound.on) Sound.ready().then(() => Sound.key(0.9));
  });
}

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
    async run(soft, from) {
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
        if (s.textContent !== ' ') Sound.key(soft, from);
        await hold(from, pause(s.textContent));
      }
      node.classList.remove('is-typing');
    },
  };
}

// A command line that types itself, key by key; `from` is the demo it belongs to.
async function typeInto(el, text, soft, from, wait = (ms) => hold(from, ms)) {
  const line = el.parentElement;
  line.classList.add('is-typing');
  for (let i = 1; i <= text.length; i++) {
    el.textContent = text.slice(0, i);
    if (text[i - 1] !== ' ') Sound.key(soft, from);
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
const at = (ms, fn) => { if (!still) setTimeout(fn, ms); };
const ASK = 'make a 60 second short';

const moments = {
  async problem(el) {
    // A soft key as each chore lands, and a tick for the last line.
    [250, 500, 750, 1000, 1250, 1500].forEach((ms) => at(ms + 60, () => Sound.key(0.5, el)));
    at(2200, () => Sound.tick(0.6, el));
    const line = el.querySelector('.cmd--answer [data-cmd]');
    const pile = el.querySelector('[data-pile]');
    if (still || !line || !pile) return;
    // One typed request clears the chores: a first glimpse of the answer.
    await hold(el, 3000);
    el.classList.add('is-asking');
    await hold(el, 300);
    await typeInto(line, 'make it polished', 0.45, el);
    await hold(el, 420);
    Sound.thock(0.45, el);
    el.classList.add('is-struck');
    [0, 120, 240, 360, 480, 600].forEach((ms) => setTimeout(() => Sound.tick(0.5, el), ms + 80));
    await hold(el, 1100);
    const gap = parseFloat(getComputedStyle(el).rowGap) || 0;
    el.style.setProperty('--fold', `${pile.offsetHeight + gap}px`);
    el.classList.add('is-folded');
  },
  async turn(el) {
    const t = typerIn(el, '[data-type]');
    if (still) { t.run(); return; }
    await hold(el, 250);
    await t.run(0.6, el);
    await hold(el, 420);
    Sound.thock(0.5, el); // the question lands like Enter
  },
  record(el) {
    [300, 600, 900, 1200].forEach((ms) => at(ms + 40, () => { Sound.tick(0.6, el); Sound.key(0.22, el); }));
  },
  async say(el) {
    await typerIn(el, '[data-ask]').run(0.45, el);
    if (still) return;
    await hold(el, 450);
    el.classList.add('is-preview');
    Sound.tick(0.45, el);
    await hold(el, 1400);
    el.classList.add('is-done');
    Sound.key(0.8, el);
    await hold(el, 260);
    const preview = el.querySelector('[data-preview]');
    if (preview) preview.textContent = 'Kept the 5 best moments. 0:58, vertical.';
    Sound.tick(0.6, el);
  },
  // The editor's screenshot: one request typed into the chat's own box. It isn't sent.
  async ask(el) {
    const line = el.querySelector('[data-typed]');
    if (still || !line) return;
    await hold(el, 700);
    el.classList.add('is-asking');
    await hold(el, 300);
    await typeInto(line, ASK, 0.4, el);
  },
  promise(el) {
    at(2300, () => Sound.key(0.85, el)); // the instant line
  },
  outcome(el) {
    at(1700, () => Sound.tick(1, el)); // the playhead reaches the end
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
  const once = (opts) => new IntersectionObserver((entries, io) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      io.unobserve(e.target);
      fire(e.target);
    }
  }, opts);
  const io = once({ rootMargin: '0px 0px -30% 0px' });
  const whole = once({ threshold: 1 }); // a small moment, such as one line, starts once all of it is on screen
  momentEls.forEach((el) => { watch(el); ('whole' in el.dataset ? whole : io).observe(el); });
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

  // Demo cuts tap only for a visitor who has touched the page and turned sound on.
  const tap = () => { if (Sound.on) Haptic.tap(4); };
  const drop = (it) => { it.gone = true; it.m.classList.add('is-gone'); };
  const said = (m) => 90 + m.textContent.length * 12 + (/\.$/.test(m.textContent) ? 220 : /,$/.test(m.textContent) ? 70 : 0);

  const steps = [
    ['cut the ums', async () => {
      for (const it of items.filter((i) => i.f && !i.gone)) {
        it.v.classList.add('is-struck');
        Sound.tick(0.7, take);
        tap();
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
        Sound.key(0.35, take);
        tap();
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
      Sound.tick(0.5, take);
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
        await typeInto(cmd, say, 0.4, take, wait);
        await wait(rand(380, 520));
        Sound.thock(0.45, take);
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

/* -- the demo film --
   Its two videos go in once the page has loaded and the stage is near, so they never compete with the first
   paint. The muted loop plays only while half of it shows and the tab is visible, and only for a visitor who
   hasn't asked for reduced motion or less data; it can be paused. "Watch the demo" plays the whole film, with
   sound and controls, in the same place. */

const unwrap = (el) => {
  for (const n of el.querySelectorAll('noscript[data-later]')) {
    n.insertAdjacentHTML('beforebegin', n.textContent);
    n.remove();
  }
};

function film(stage) {
  const play = stage.querySelector('[data-play]');
  const loop = d.querySelector('[data-loop]');
  const thrifty = Boolean(navigator.connection && navigator.connection.saveData);
  let teaser = null;
  let full = null;
  let choice = null; // the visitor's own Pause or Play for the loop; until then reduced motion and data saving decide
  let shown = false; // half of the stage is on screen
  let started = false; // the film was asked for

  const wanted = () => (choice === null ? !still && !thrifty : choice);
  const label = () => { if (loop) loop.textContent = wanted() ? 'Pause the preview' : 'Play the preview'; };
  const sync = () => {
    if (!teaser || started) return;
    if (wanted() && shown && !d.hidden) {
      if (teaser.paused) teaser.play()?.catch(() => {}); // refused (a phone's low power mode): the poster stays
    } else if (!teaser.paused) teaser.pause();
  };
  const mount = () => {
    if (teaser) return;
    unwrap(stage);
    teaser = stage.querySelector('[data-teaser]');
    full = stage.querySelector('[data-full]');
    teaser.muted = true;
    teaser.addEventListener('click', start);
    sync();
  };
  function start() {
    mount();
    started = true;
    teaser.pause();
    stage.classList.add('is-film');
    play.hidden = true;
    if (loop) loop.hidden = true;
    full.tabIndex = 0;
    full.focus({ preventScroll: true });
    full.play()?.catch(() => {}); // if refused, the film's own controls are there to press
  }

  play.hidden = false;
  play.addEventListener('click', start);
  if (loop) {
    label();
    loop.hidden = false;
    loop.addEventListener('click', () => { choice = !wanted(); label(); sync(); });
  }
  reduce.addEventListener?.('change', () => { label(); sync(); });
  d.addEventListener('visibilitychange', sync);

  if (!('IntersectionObserver' in window)) {
    shown = true;
    mount();
    return;
  }
  new IntersectionObserver((es) => {
    for (const e of es) shown = e.isIntersecting && e.intersectionRatio >= 0.5;
    sync();
  }, { threshold: [0, 0.5, 1] }).observe(stage);
  const near = new IntersectionObserver((es) => {
    if (!es.some((e) => e.isIntersecting)) return;
    near.disconnect();
    mount();
  }, { rootMargin: '100% 0px' });
  const idle = (fn) => (window.requestIdleCallback ? requestIdleCallback(fn, { timeout: 2000 }) : setTimeout(fn, 200));
  const ready = () => idle(() => near.observe(stage));
  if (d.readyState === 'complete') ready();
  else addEventListener('load', ready, { once: true });
}

const stage = d.querySelector('[data-film]');
if (stage) film(stage);
