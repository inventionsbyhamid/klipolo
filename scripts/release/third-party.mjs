// Klipolo's third-party notices, printed to stdout (scripts/release/third-party.sh runs it and says where they go):
//   node third-party.mjs <cargo metadata JSON> <page folder> <licence texts folder>
// The Rust crates: what the app crate's binary links, following normal dependencies from klipolo-app (build scripts'
// and tests' dependencies don't ship), for the platforms cargo metadata was filtered to, and the Rust standard
// library. Klipolo's own crates are left out. The JavaScript packages: every package the page's Vite build reads (run
// in memory, nothing is written), the packages its stylesheets @import, and the bundler's own runtime helpers that end
// up in the page (Vite's and Rolldown's). Each package takes the first licence in PREFERRED that it offers; a licence
// needs its text in the texts folder (<SPDX id>.txt) to be taken at all, so a package with no such licence stops the
// run. With the bundled pack's manifest (assets/library/manifest.json) as a fourth argument, the pack's sounds and
// pictures made by others (its items with an author) are listed too, each under its licence, which needs a text there
// as well.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';

const PREFERRED = [
  'MIT', 'Apache-2.0', 'ISC', 'BSD-3-Clause', 'BSD-2-Clause', 'Zlib', '0BSD', 'Unicode-3.0', 'MPL-2.0', 'CC0-1.0',
  'CDLA-Permissive-2.0', 'WTFPL',
];
// Licences that can't be shipped in an AGPL-3.0-only app, whatever texts are in the folder: a package offering only
// these stops the run, and a text for one of them is refused.
const DENIED = ['GPL-2.0-only', 'GPL-2.0', 'LGPL-2.0-only', 'LGPL-2.0'];
const RULE = '-'.repeat(100);

const [metadataFile, pageDir, textsDir, packManifest] = process.argv.slice(2);
if (!textsDir) {
  console.error('usage: node third-party.mjs <cargo metadata JSON> <page folder> <licence texts folder> [pack manifest]');
  process.exit(2);
}
const texts = new Map(
  fs.readdirSync(textsDir).filter((f) => f.endsWith('.txt')).map((f) => [f.slice(0, -4), tidy(read(path.join(textsDir, f)))]),
);
const problems = [];
for (const id of DENIED) {
  if (texts.has(id)) {
    console.error(`refused: ${textsDir}/${id}.txt: ${id} can't ship in an AGPL-3.0-only app, so it has no text there`);
    process.exit(1);
  }
}

const rust = rustPackages(JSON.parse(read(metadataFile)));
const page = await pagePackages(path.resolve(pageDir));
const all = [...rust, ...page];
for (const p of all) {
  p.files = licenceFiles(p.dirs ?? [p.dir], p.licenseFile && path.resolve(p.dir, p.licenseFile));
  p.chosen = choose(p);
  p.copyright = unique(p.files.filter((f) => !/^notice/i.test(path.basename(f))).flatMap((f) => copyrightLines(read(f))));
  p.notices = p.files.filter((f) => /^notice/i.test(path.basename(f))).map((f) => tidy(read(f)));
}
const pack = packManifest
  ? JSON.parse(read(packManifest)).items.filter((i) => i.author).map((i) => ({ ...i, kind: 'pack' }))
  : [];
for (const i of pack) {
  if (DENIED.includes(i.license) || !texts.has(i.license)) {
    problems.push(`the pack's ${i.file} is under "${i.license}", which has no text in scripts/release/licenses`);
  }
}
if (problems.length) {
  console.error(problems.map((p) => `refused: ${p}`).join('\n'));
  console.error(
    'Check that each licence can ship in an AGPL-3.0-only app before adding its SPDX text to scripts/release/licenses.',
  );
  process.exit(1);
}
process.stdout.write(render(rust, page, pack));

// ---------------------------------------------------------------------------------------------------------------------

function rustPackages(meta) {
  const byId = new Map(meta.packages.map((p) => [p.id, p]));
  const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
  const own = new Set(meta.workspace_members);
  const root = meta.packages.find((p) => p.name === 'klipolo-app' && own.has(p.id));
  if (!root) throw new Error('no klipolo-app in the cargo metadata');
  const seen = new Set();
  const todo = [root.id];
  while (todo.length) {
    const id = todo.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const d of nodes.get(id).deps) if (d.dep_kinds.some((k) => k.kind === null)) todo.push(d.pkg);
  }
  return [rustStd(meta.workspace_root), ...[...seen]
    .filter((id) => !own.has(id))
    .map((id) => byId.get(id))
    .map((p) => {
      const dir = path.dirname(p.manifest_path);
      const git = gitSource(p.source);
      return {
        kind: 'rust',
        name: p.name,
        version: p.version,
        license: p.license,
        licenseFile: p.license_file,
        // A git dependency is the repository and commit Cargo.lock pins (a fork, often), not the crate's own
        // `repository`, and its licence files may sit at the top of that repository rather than in the crate's folder.
        url: git ? git.url : p.repository || p.homepage || `https://crates.io/crates/${p.name}`,
        commit: git?.commit,
        authors: p.authors ?? [],
        dir,
        dirs: git ? upToCheckout(dir) : [dir],
      };
    })];
}

// The Rust standard library, which every Rust binary links but cargo metadata doesn't list: the toolchain's version,
// from rust-toolchain.toml.
function rustStd(root) {
  const m = /^channel\s*=\s*"([^"]+)"/m.exec(read(path.join(root, 'rust-toolchain.toml')));
  if (!m) throw new Error('no channel in rust-toolchain.toml');
  return {
    kind: 'rust',
    name: 'Rust standard library',
    version: m[1],
    license: 'MIT OR Apache-2.0',
    url: 'https://github.com/rust-lang/rust',
    authors: ['The Rust Project Developers'],
    dirs: [],
  };
}

// "git+https://github.com/o/r?rev=abc#<commit>" as { url: "https://github.com/o/r", commit }; null for other sources.
function gitSource(source) {
  const m = /^git\+([^?#]+)(?:\?[^#]*)?#([0-9a-f]{40})$/.exec(source ?? '');
  if (!m) {
    if ((source ?? '').startsWith('git+')) throw new Error(`a git source without a full commit: ${source}`);
    return null;
  }
  return { url: m[1].replace(/\.git$/, ''), commit: m[2] };
}

// <dir> and its parents up to the root of Cargo's git checkout (the folder holding .cargo-ok), nearest first.
function upToCheckout(dir) {
  const dirs = [dir];
  for (let d = dir; !fs.existsSync(path.join(d, '.cargo-ok')); ) {
    const up = path.dirname(d);
    if (up === d) throw new Error(`no git checkout root above ${dir}`);
    d = up;
    dirs.push(d);
  }
  return dirs;
}

async function pagePackages(dir) {
  const require = createRequire(path.join(dir, 'package.json'));
  const vite = await import(pathToFileURL(require.resolve('vite')).href);
  const ids = new Set();
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'klipolo-third-party-'));
  try {
    await vite.build({
      root: dir,
      configFile: path.join(dir, 'vite.config.ts'),
      logLevel: 'error',
      build: { outDir: out, emptyOutDir: true, write: false },
      plugins: [
        {
          name: 'klipolo-third-party',
          generateBundle() {
            for (const id of this.getModuleIds()) ids.add(id);
          },
        },
      ],
    });
  } finally {
    fs.rmSync(out, { recursive: true, force: true });
  }
  if (!ids.size) throw new Error('the page build read no modules');

  const roots = new Set();
  const nm = `${path.sep}node_modules${path.sep}`;
  for (const id of ids) {
    if (id.startsWith('\0')) {
      // Plugins' virtual modules: Vite's preload helper and polyfill are Vite's code; anything else is unknown.
      if (id.startsWith('\0vite/')) roots.add(installed(dir, 'vite'));
      else problems.push(`the page build has a virtual module from an unknown package: ${JSON.stringify(id)}`);
      continue;
    }
    const file = id.split('?')[0];
    const at = file.lastIndexOf(nm);
    if (at < 0) continue; // the page's own code
    const parts = file.slice(at + nm.length).split(path.sep);
    const name = parts[0].startsWith('@') ? `${parts[0]}/${parts[1]}` : parts[0];
    roots.add(file.slice(0, at + nm.length) + name);
  }
  // Stylesheets' @import of a package (Tailwind's theme and utilities): compiled into the page's CSS by the Tailwind
  // plugin, so not a module the build reports.
  for (const css of walk(path.join(dir, 'src')).filter((f) => f.endsWith('.css'))) {
    for (const m of read(css).matchAll(/@import\s+["']([^"'./][^"']*)["']/g)) {
      const parts = m[1].split('/');
      roots.add(installed(dir, parts[0].startsWith('@') ? `${parts[0]}/${parts[1]}` : parts[0]));
    }
  }
  // Rolldown's runtime helpers (CommonJS interop) are written into the page by Vite's bundler.
  const viteRoot = installed(dir, 'vite');
  roots.add(viteRoot);
  roots.add(installed(viteRoot, 'rolldown', true));

  return [...roots].map((root) => {
    const pkg = JSON.parse(read(path.join(root, 'package.json')));
    return {
      kind: 'js',
      name: pkg.name,
      version: pkg.version,
      license: jsLicense(pkg),
      url: repoUrl(pkg) || pkg.homepage || `https://www.npmjs.com/package/${pkg.name}`,
      authors: [pkg.author].flat().filter(Boolean).map((a) => (typeof a === 'string' ? a : a.name)).filter(Boolean),
      dir: root,
    };
  });
}

// The real folder of package <name> as the package or page at <from> sees it: <from>/node_modules/<name>, or with
// `sibling` next to <from> itself, where pnpm puts a package's dependencies.
function installed(from, name, sibling = false) {
  const candidates = [path.join(from, 'node_modules', name)];
  const up = path.basename(path.dirname(from)).startsWith('@') ? ['..', '..'] : ['..'];
  if (sibling) candidates.push(path.join(from, ...up, name));
  for (const c of candidates) {
    if (fs.existsSync(path.join(c, 'package.json'))) return fs.realpathSync(c);
  }
  throw new Error(`no package ${name} installed for ${from} (pnpm install)`);
}

function jsLicense(pkg) {
  if (typeof pkg.license === 'string') return pkg.license;
  if (pkg.license?.type) return pkg.license.type;
  if (Array.isArray(pkg.licenses)) return pkg.licenses.map((l) => l.type ?? l).join(' OR ');
  return null;
}

function repoUrl(pkg) {
  const r = typeof pkg.repository === 'string' ? pkg.repository : pkg.repository?.url;
  if (!r) return null;
  if (/^[\w.-]+\/[\w.-]+$/.test(r)) return `https://github.com/${r}`; // npm's "user/repo" shorthand
  return r.replace(/^git\+/, '').replace(/^git:\/\//, 'https://').replace(/\.git$/, '').replace(/^github:/, 'https://github.com/');
}

// The licence, copyright and notice files in each of <dirs> (the package's folder, and for a git dependency the
// folders above it up to its repository's top), and Cargo's license-file if it names another.
function licenceFiles(dirs, licenseFile) {
  const files = dirs.flatMap((dir) =>
    fs
      .readdirSync(dir)
      .filter((f) => /^(licen[cs]e|copying|copyright|notice|unlicense)([-._].*)?$/i.test(f))
      .sort()
      .map((f) => path.join(dir, f))
      .filter((f) => fs.statSync(f).isFile()),
  );
  if (licenseFile && fs.existsSync(licenseFile) && !files.includes(licenseFile)) files.push(licenseFile);
  return files;
}

// The package's licence expression as the sets of licences it allows (OR of ANDs), and the set Klipolo takes: every
// licence in it has a text, and its least preferred licence is the most preferred of all the allowed sets'.
function choose(p) {
  const label = `${p.kind === 'rust' ? 'crate' : 'package'} ${p.name} ${p.version}`;
  if (!p.license) {
    problems.push(`${label} names no SPDX licence`);
    return [];
  }
  let options;
  try {
    options = parse(p.license);
  } catch (e) {
    problems.push(`${label}: can't read its licence "${p.license}": ${e.message}`);
    return [];
  }
  const rank = (id) => (PREFERRED.includes(id) ? PREFERRED.indexOf(id) : PREFERRED.length);
  const denied = (id) => DENIED.includes(id.split(' WITH ')[0]);
  const usable = options.filter((o) => o.every((id) => texts.has(id) && !denied(id)));
  if (!usable.length) {
    const deniedIds = unique(options.flat().filter(denied));
    problems.push(
      deniedIds.length
        ? `${label} is under "${p.license}": ${deniedIds.join(', ')} can't ship in an AGPL-3.0-only app`
        : `${label} is under "${p.license}": none of it has a text in scripts/release/licenses`,
    );
    return [];
  }
  usable.sort((a, b) => Math.max(...a.map(rank)) - Math.max(...b.map(rank)) || a.length - b.length);
  return [...usable[0]].sort();
}

// An SPDX expression (with Cargo's old "A/B" for "A OR B") as a list of options, each a list of licence ids.
// "A WITH B" is one id needing both texts' names joined, so it is taken only if "A WITH B".txt exists.
function parse(expr) {
  const tokens = expr.replace(/\//g, ' OR ').match(/\(|\)|[^\s()]+/g) ?? [];
  let i = 0;
  const peek = () => tokens[i];
  const word = (w) => peek() && peek().toUpperCase() === w;
  function or() {
    let left = and();
    while (word('OR')) {
      i++;
      left = [...left, ...and()];
    }
    return left;
  }
  function and() {
    let left = atom();
    while (word('AND')) {
      i++;
      const right = atom();
      left = left.flatMap((a) => right.map((b) => [...new Set([...a, ...b])]));
    }
    return left;
  }
  function atom() {
    const t = tokens[i++];
    if (t === undefined) throw new Error('it ends too soon');
    if (t === '(') {
      const inner = or();
      if (tokens[i++] !== ')') throw new Error('a bracket isn\'t closed');
      return inner;
    }
    if (t === ')' || ['AND', 'OR', 'WITH'].includes(t.toUpperCase())) throw new Error(`"${t}" is out of place`);
    if (word('WITH')) {
      i++;
      return [[`${t} WITH ${tokens[i++]}`]];
    }
    return [[t]];
  }
  const result = or();
  if (i !== tokens.length) throw new Error(`"${tokens[i]}" is out of place`);
  return result;
}

// Copyright lines: "Copyright ...", "(c) 2020 ..." or "© ...", leaving out licence templates' placeholders, the
// licence texts' own sentences about copyright, and the FSF's copyright on the GPL's own text (a package offering the
// GPL as one choice ships that text).
function copyrightLines(text) {
  const placeholder =
    /<(year|yyyy|copyright holders?|owner|organization)>|\[(yyyy|year|name of copyright owner|fullname)\]|\{(yyyy|year|name of copyright owner)\}/i;
  return text
    .split(/\r?\n/)
    .map((l) => l.trim().replace(/\s+/g, ' '))
    .filter((l) => /^(Copyright\b|\([cC]\) ?\d|©)/.test(l))
    .filter((l) => !/^Copyright (notice|owner|holder|and|law|statement|license|licence)/i.test(l))
    .filter((l) => !placeholder.test(l))
    .filter((l) => !/Free Software Foundation/.test(l));
}

function render(rust, page, pack) {
  const byLicence = new Map();
  for (const p of [...rust, ...page]) {
    for (const id of p.chosen) {
      if (!byLicence.has(id)) byLicence.set(id, []);
      byLicence.get(id).push(p);
    }
  }
  const ids = [...byLicence.keys()].sort();
  const order = (a, b) => cmp(a.name, b.name) || cmp(a.version, b.version);
  const lines = [
    'Klipolo: third-party notices',
    '',
    'Klipolo for macOS is built with the open source Rust crates and JavaScript packages listed here. Each is used',
    'under the licence it is listed under. When a package offers a choice of licences, its full choice is shown in',
    'brackets, and Klipolo takes the one it is listed under. Each licence\'s full text is given once, after the',
    'packages that use it. The copyright lines are the ones in each package\'s own licence files; a package whose files',
    'have none shows its authors instead. Each package\'s source code is at the address listed with it.',
    '',
    'Klipolo\'s own licence, and the licences of FFmpeg and the fonts, are in NOTICE.txt and the files it names. The',
    'sounds and pictures of Klipolo\'s bundled pack made by others are listed at the end, each under its licence.',
    '',
    'This file is written by scripts/release/third-party.sh from Cargo.lock, the page\'s build and the pack\'s manifest.',
    'Don\'t edit it by hand.',
    '',
    `${rust.length} Rust crates and ${page.length} JavaScript packages, under these licences:`,
    ...ids.map((id) => `  ${id.padEnd(24)} ${String(byLicence.get(id).length).padStart(4)}`),
  ];
  for (const id of ids) {
    const users = byLicence.get(id);
    lines.push('', RULE, `${id} (${users.length})`, RULE);
    for (const [kind, title] of [['rust', 'Rust crates'], ['js', 'JavaScript packages']]) {
      const group = users.filter((p) => p.kind === kind).sort(order);
      if (!group.length) continue;
      lines.push('', title);
      for (const p of group) {
        const choice = p.license === id ? '' : ` (${p.license})`;
        lines.push(`  ${p.name} ${p.version}${choice}  ${p.url}${p.commit ? ` at commit ${p.commit}` : ''}`);
        if (p.copyright.length) lines.push(...p.copyright.map((c) => `      ${c}`));
        else if (p.authors.length) lines.push(`      Authors: ${p.authors.join(', ')}`);
      }
    }
    lines.push('', `The ${id} licence:`, '', texts.get(id));
  }
  if (pack.length) {
    const licences = unique(pack.map((i) => i.license)).sort();
    lines.push(
      '',
      RULE,
      `The bundled pack's sounds and pictures made by others (${pack.length})`,
      RULE,
      '',
      'Klipolo.app/Contents/Resources/library holds Klipolo\'s pack. These files of it were made by others: each is used',
      'under the licence it is listed under, and the pack\'s own LICENSE.txt credits each one and says how it was changed.',
    );
    for (const id of licences) {
      lines.push('', `${id} (${pack.filter((i) => i.license === id).length})`);
      for (const i of pack.filter((i) => i.license === id)) lines.push(`  ${i.file}  by ${i.author}  ${i.source}`);
    }
    for (const id of licences.filter((id) => !byLicence.has(id))) lines.push('', `The ${id} licence:`, '', texts.get(id));
    const given = licences.filter((id) => byLicence.has(id));
    if (given.length) lines.push('', `The ${given.join(' and ')} licence texts are given above.`);
  }
  const noticed = [...rust, ...page].filter((p) => p.notices.length).sort(order);
  if (noticed.length) {
    lines.push('', RULE, 'NOTICE files, passed on as their licences ask', RULE);
    for (const p of noticed) lines.push('', `${p.name} ${p.version}:`, '', ...p.notices);
  }
  return `${tidy(lines.join('\n'))}\n`;
}

// ---------------------------------------------------------------------------------------------------------------------

function read(file) {
  return fs.readFileSync(file, 'utf8');
}

// Unix line ends, no trailing spaces, no blank lines at either end.
function tidy(text) {
  return text
    .replace(/^﻿/, '')
    .replace(/\r\n?/g, '\n')
    .split('\n')
    .map((l) => l.replace(/\s+$/, ''))
    .join('\n')
    .replace(/^\n+|\n+$/g, '');
}

function unique(list) {
  return [...new Set(list)];
}

function cmp(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

function walk(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const f = path.join(dir, e.name);
    return e.isDirectory() ? walk(f) : [f];
  });
}
