import { useLingui } from '@lingui/react/macro';
import { useEffect, useRef, useState, type PointerEvent, type ReactNode } from 'react';
import type { Asset } from '../../bindings/Asset';
import type { PackItem } from '../../bindings/PackItem';
import type { Project } from '../../bindings/Project';
import type { LibraryMarks } from '../../bridge/library';
import { Icon } from '../../layout/Icon';
import { useProject } from '../../state/project';
import type { LibraryTab } from '../../state/ui';
import { beginMediaDrag } from '../timeline/drag/mediaDrag';
import { addMedia } from './addMedia';
import { AssetMenu } from './AssetMenu';
import { AssetRow } from './AssetRow';
import { FavoriteButton, type Favorite } from './FavoriteButton';
import { fileKey, loadMarks, marked, markUsed, markWhenAdded, packKey, setFavorite, useMarks } from './marks';
import { addImage, addMusic, addPackItem, addSound, dropPackItem } from './place';
import { searchLibrary, type Searchable } from './search';
import { loadPack, packAsset, packAssetId, shelves, useNearView, usePack, usePicture, uses } from './shelves';

/** On a Mac, Control-click is a right-click: it doesn't start a drag. */
const MAC = typeof navigator !== 'undefined' && /^Mac/.test(navigator.platform);

/** A group's heading; `id` names the list under it (`aria-labelledby`). */
function Group({ title, note, id }: { title: string; note?: string; id?: string }) {
  return (
    <p className="group-title" id={id}>
      {title}
      {note && <span>{note}</span>}
    </p>
  );
}

/**
 * A picture in the Images grid: on a checkerboard (it may be see-through), its name under it; "+" adds it, and its star
 * (top left) makes it a favorite.
 */
function Picture({
  asset,
  from,
  title,
  onAdd,
  onDrop,
  favorite,
}: {
  asset: Asset;
  from: { packId: string } | { path: string };
  title: string;
  onAdd: () => void;
  onDrop: Parameters<typeof beginMediaDrag>[2];
  favorite: Favorite;
}) {
  const { t } = useLingui();
  const tile = useRef<HTMLLIElement>(null);
  const url = usePicture(from, useNearView(tile));
  const onPointerDown = (e: PointerEvent<HTMLElement>) => {
    if (e.button !== 0 || (MAC && e.ctrlKey) || (e.target as Element).closest('button')) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture?.(e.pointerId);
    beginMediaDrag(e, asset, onDrop);
  };
  return (
    <li
      ref={tile}
      className="picture"
      data-asset-id={asset.id}
      onPointerDown={onPointerDown}
      onDragStart={(e) => e.preventDefault()}
    >
      <span className="picture-art checker">{url && <img src={url} alt="" draggable={false} />}</span>
      <span className="picture-name" title={title}>
        {title}
      </span>
      <button
        type="button"
        className="icon-btn row-action"
        aria-label={t({
          id: 'library.add',
          message: `Add ${title} at the playhead`,
          comment: '{title} is a file or a sound’s name',
        })}
        title={t({ id: 'library.add.tip', message: 'Add at the playhead' })}
        onClick={onAdd}
      >
        <Icon name="plus" />
      </button>
      <FavoriteButton title={title} favorite={favorite} />
    </li>
  );
}

/** What a music row's "+" says: music goes under the whole video. */
function musicAddLabel(t: ReturnType<typeof useLingui>['t'], name: string): string {
  return t({ id: 'library.add.music', message: `Add ${name} under the whole video`, comment: '{name} is a song’s name' });
}

/** Item `key`'s star, from the marks. */
function favoriteOf(marks: LibraryMarks, key: string): Favorite {
  const on = marks[key]?.favorite ?? false;
  return { on, toggle: () => setFavorite(key, !on) };
}

/** The pack's items of one kind, as rows or pictures. Each one the library adds is recently used. */
function PackItems({ items, project }: { items: PackItem[]; project: Project }) {
  const { t } = useLingui();
  const marks = useMarks((s) => s.marks);
  const used = uses(project).any;
  return items.map((item) => {
    const key = packKey(item.id);
    const favorite = favoriteOf(marks, key);
    const add = () => void markWhenAdded(key, addPackItem(item));
    const drop = (target: Parameters<typeof dropPackItem>[1]) => void markWhenAdded(key, dropPackItem(item, target));
    return item.category === 'sticker' ? (
      <Picture
        key={item.id}
        asset={packAsset(item)}
        from={{ packId: item.id }}
        title={item.name}
        onAdd={add}
        onDrop={drop}
        favorite={favorite}
      />
    ) : (
      <AssetRow
        key={item.id}
        asset={packAsset(item)}
        art={item.category === 'music' ? 'music' : 'sound'}
        title={item.name}
        used={used.has(packAssetId(item.id))}
        onAdd={add}
        addLabel={item.category === 'music' ? musicAddLabel(t, item.name) : undefined}
        onDrop={drop}
        favorite={favorite}
      />
    );
  });
}

/** How the search reads a pack item: its tags, and "Built in". */
function aboutPack(item: PackItem): Searchable {
  const kind = item.category === 'sticker' ? 'image' : 'audio';
  return { name: item.name, tags: item.tags, category: item.category, kind, collection: 'Built in' };
}

/** How the search reads one of the person's files on a tab: as `category`, from `collection` ("Your sounds"). */
const aboutOwn =
  (category: string, collection: string) =>
  (asset: Asset): Searchable => ({ name: asset.name, tags: [], category, kind: asset.kind, collection });

const ownKey = (asset: Asset) => fileKey(asset.path);
const packItemKey = (item: PackItem) => packKey(item.id);

/**
 * The marks a tab's lists are in order by: as they were when the tab, the search or the marks read from Rust last
 * changed, or when the pointer or the focus last left the lists (`settle`). A star or an add shows on its own row at
 * once but moves no row while the person is on the list, so the next click lands on the item they aimed at, focus
 * stays on the star they pressed, and a drag that began meanwhile goes on.
 */
function useSettledMarks(tab: LibraryTab, query: string) {
  const reads = useMarks((s) => s.reads);
  const key = `${tab}\n${query}\n${reads}`;
  const [settled, setSettled] = useState(() => ({ key, marks: useMarks.getState().marks }));
  let marks = settled.marks;
  if (settled.key !== key) {
    marks = useMarks.getState().marks;
    setSettled({ key, marks });
  }
  const settle = () => {
    const now = useMarks.getState().marks;
    setSettled((s) => (s.marks === now ? s : { key: s.key, marks: now }));
  };
  return { marks, settle };
}

/**
 * What the Library shows on one tab: your files first, then Klipolo's pack where it has some. Clips are
 * the project's videos; Music and Sounds the sounds (by how they play, else by length) and the pack's beds and effects;
 * Images a 3-column grid of your pictures, those made with Muse Image, and the pack's stickers. Drag any row onto the
 * lane it belongs to; the "+" adds it at the playhead. `query` (the panel's search) keeps the items it finds, best
 * first (`search.ts`); in each list, favorites come first, then what the library added most recently (`marks.ts`),
 * which also decide between equal matches. The order follows the marks once the person has left the list
 * (`useSettledMarks`).
 */
export function MediaList({ tab, query }: { tab: LibraryTab; query: string }) {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  const pack = usePack((s) => s.items);
  const marks = useMarks((s) => s.marks);
  const order = useSettledMarks(tab, query);
  useEffect(loadPack, []);
  useEffect(loadMarks, []);
  if (!project) return null;
  /** The tab's lists: the order catches up with the marks when the pointer or the focus leaves them. */
  const lists = (content: ReactNode) => (
    <div
      className="lib-lists"
      onPointerLeave={order.settle}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget)) order.settle();
      }}
    >
      {content}
    </div>
  );
  const own = shelves(project);
  const used = uses(project).any;
  const builtIn = t({ id: 'library.builtIn', message: 'Built in' });
  const free = t({ id: 'library.builtIn.free', message: 'Free to use' });
  const found = <T,>(items: T[], about: (item: T) => Searchable, keyOf: (item: T) => string) =>
    searchLibrary(marked(items, keyOf, order.marks), query, about);
  const ofPack = (category: PackItem['category']) =>
    found(
      pack.filter((item) => item.category === category),
      aboutPack,
      packItemKey,
    );
  const asked = query.trim();
  /** The one line a search that finds nothing on this tab shows. */
  const none = (text: string) => (
    <p className="lib-none" role="status">
      {text}
    </p>
  );
  const row = (asset: Asset, art: 'music' | 'sound', add: () => Promise<boolean>) => (
    <AssetRow
      key={asset.id}
      asset={asset}
      art={art}
      title={asset.name}
      used={used.has(asset.id)}
      onAdd={() => void markWhenAdded(ownKey(asset), add())}
      addLabel={art === 'music' ? musicAddLabel(t, asset.name) : undefined}
      onDrop={(target) => addMedia(asset.id, target, () => markUsed(ownKey(asset)))}
      favorite={favoriteOf(marks, ownKey(asset))}
    />
  );

  if (tab === 'clips') {
    if (own.clips.length === 0)
      return lists(
        <div className="lib-empty">
          <p className="lib-empty-title">{t({ id: 'library.clips.empty', message: 'No clips yet' })}</p>
          <p>
            {t({
              id: 'library.clips.emptyText',
              message: 'Clips you add show up here. Ask chat for the best moments once Klipolo has listened to them.',
            })}
          </p>
        </div>
      );
    const clips = found(own.clips, aboutOwn('video', 'Your clips'), ownKey);
    if (clips.length === 0)
      return lists(
        none(t({ id: 'library.clips.noMatch', message: `No clips match “${asked}”.`, comment: '{asked} is what the person searched for' })),
      );
    return lists(
      <ul className="lib-list" aria-label={t({ id: 'library.clips', message: 'Your clips' })}>
        {clips.map((clip) => (
          <AssetRow
            key={clip.id}
            asset={clip}
            art="clip"
            title={clip.name}
            used={used.has(clip.id)}
            onAdd={() =>
              addMedia(clip.id, { lane: 'main', index: project.timeline.main.length }, () => markUsed(ownKey(clip)))
            }
            addLabel={t({ id: 'library.add.end', message: `Add ${clip.name} to the end`, comment: '{clip.name} is a file name' })}
            onDrop={(target) => addMedia(clip.id, target, () => markUsed(ownKey(clip)))}
            menu={<AssetMenu asset={clip} />}
            favorite={favoriteOf(marks, ownKey(clip))}
          />
        ))}
      </ul>
    );
  }

  if (tab === 'music' || tab === 'sounds') {
    const music = tab === 'music';
    const mine = found(
      music ? own.music : own.sounds,
      aboutOwn(music ? 'music' : 'sound', music ? 'Your music' : 'Your sounds'),
      ownKey,
    );
    const packed = ofPack(music ? 'music' : 'sound');
    return lists(
      <>
        {mine.length > 0 && (
          <>
            <Group
              id={`lib-${tab}-yours`}
              title={music ? t({ id: 'library.music.yours', message: 'Your music' }) : t({ id: 'library.sounds.yours', message: 'Your sounds' })}
            />
            <ul className="lib-list" aria-labelledby={`lib-${tab}-yours`}>
              {mine.map((asset) =>
                row(asset, music ? 'music' : 'sound', () => (music ? addMusic : addSound)({ type: 'asset', id: asset.id })),
              )}
            </ul>
          </>
        )}
        {packed.length > 0 && (
          <>
            <Group id={`lib-${tab}-pack`} title={builtIn} note={free} />
            <ul className="lib-list" aria-labelledby={`lib-${tab}-pack`}>
              <PackItems items={packed} project={project} />
            </ul>
          </>
        )}
        {asked &&
          mine.length === 0 &&
          packed.length === 0 &&
          none(
            music
              ? t({ id: 'library.music.noMatch', message: `No music matches “${asked}”.`, comment: '{asked} is what the person searched for' })
              : t({ id: 'library.sounds.noMatch', message: `No sounds match “${asked}”.`, comment: '{asked} is what the person searched for' }),
          )}
        <p className="lib-foot">
          {music
            ? t({
                id: 'library.music.foot',
                message: 'Drop MP3, M4A or WAV files here. Music goes under the whole video and gets quieter while people talk.',
              })
            : t({ id: 'library.sounds.foot', message: 'Drag a sound onto the timeline, or ask in chat.' })}
        </p>
      </>
    );
  }

  const picture = (asset: Asset) => (
    <Picture
      key={asset.id}
      asset={asset}
      from={{ path: asset.path }}
      title={asset.name}
      onAdd={() => void markWhenAdded(ownKey(asset), addImage({ type: 'asset', id: asset.id }))}
      onDrop={(target) => addMedia(asset.id, target, () => markUsed(ownKey(asset)))}
      favorite={favoriteOf(marks, ownKey(asset))}
    />
  );
  const images = found(own.images, aboutOwn('photo', 'Your images'), ownKey);
  const made = found(own.made, aboutOwn('photo', 'Made with Muse Image'), ownKey);
  const stickers = ofPack('sticker');
  return lists(
    <>
      {images.length > 0 && (
        <>
          <Group id="lib-images-yours" title={t({ id: 'library.images.yours', message: 'Your images' })} />
          <ul className="image-grid" aria-labelledby="lib-images-yours">
            {images.map(picture)}
          </ul>
        </>
      )}
      {made.length > 0 && (
        <>
          <Group id="lib-images-made" title={t({ id: 'library.images.made', message: 'Made with Muse Image' })} />
          <ul className="image-grid" aria-labelledby="lib-images-made">
            {made.map(picture)}
          </ul>
        </>
      )}
      {stickers.length > 0 && (
        <>
          <Group id="lib-images-pack" title={builtIn} note={free} />
          <ul className="image-grid" aria-labelledby="lib-images-pack">
            <PackItems items={stickers} project={project} />
          </ul>
        </>
      )}
      {asked &&
        images.length + made.length + stickers.length === 0 &&
        none(t({ id: 'library.images.noMatch', message: `No images match “${asked}”.`, comment: '{asked} is what the person searched for' }))}
      <p className="lib-foot">
        {t({
          id: 'library.images.foot',
          message: 'Drop PNG or JPG files here, or ask in chat for a sticker.',
        })}
      </p>
    </>
  );
}
