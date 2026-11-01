import { useLingui } from '@lingui/react/macro';
import { useEffect, useRef, type PointerEvent } from 'react';
import type { Asset } from '../../bindings/Asset';
import type { PackItem } from '../../bindings/PackItem';
import type { Project } from '../../bindings/Project';
import { Icon } from '../../layout/Icon';
import { useProject } from '../../state/project';
import type { LibraryTab } from '../../state/ui';
import { beginMediaDrag } from '../timeline/drag/mediaDrag';
import { addMedia } from './addMedia';
import { AssetMenu } from './AssetMenu';
import { AssetRow } from './AssetRow';
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

/** A picture in the Images grid: on a checkerboard (it may be see-through), its name under it; "+" adds it. */
function Picture({
  asset,
  from,
  title,
  onAdd,
  onDrop,
}: {
  asset: Asset;
  from: { packId: string } | { path: string };
  title: string;
  onAdd: () => void;
  onDrop: Parameters<typeof beginMediaDrag>[2];
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
    </li>
  );
}

/** What a music row's "+" says: music goes under the whole video. */
function musicAddLabel(t: ReturnType<typeof useLingui>['t'], name: string): string {
  return t({ id: 'library.add.music', message: `Add ${name} under the whole video`, comment: '{name} is a song’s name' });
}

/** The pack's items of one kind, as rows or pictures. */
function PackItems({ items, project }: { items: PackItem[]; project: Project }) {
  const { t } = useLingui();
  const used = uses(project).any;
  return items.map((item) =>
    item.category === 'sticker' ? (
      <Picture
        key={item.id}
        asset={packAsset(item)}
        from={{ packId: item.id }}
        title={item.name}
        onAdd={() => void addPackItem(item)}
        onDrop={(target) => void dropPackItem(item, target)}
      />
    ) : (
      <AssetRow
        key={item.id}
        asset={packAsset(item)}
        art={item.category === 'music' ? 'music' : 'sound'}
        title={item.name}
        used={used.has(packAssetId(item.id))}
        onAdd={() => void addPackItem(item)}
        addLabel={item.category === 'music' ? musicAddLabel(t, item.name) : undefined}
        onDrop={(target) => void dropPackItem(item, target)}
      />
    ),
  );
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

/**
 * What the Library shows on one tab: your files first, then Klipolo's pack where it has some. Clips are
 * the project's videos; Music and Sounds the sounds (by how they play, else by length) and the pack's beds and effects;
 * Images a 3-column grid of your pictures, those made with Muse Image, and the pack's stickers. Drag any row onto the
 * lane it belongs to; the "+" adds it at the playhead. `query` (the panel's search) keeps the items it finds, best
 * first (`search.ts`).
 */
export function MediaList({ tab, query }: { tab: LibraryTab; query: string }) {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  const pack = usePack((s) => s.items);
  useEffect(loadPack, []);
  if (!project) return null;
  const own = shelves(project);
  const used = uses(project).any;
  const builtIn = t({ id: 'library.builtIn', message: 'Built in' });
  const free = t({ id: 'library.builtIn.free', message: 'Free to use' });
  const ofPack = (category: PackItem['category']) =>
    searchLibrary(
      pack.filter((item) => item.category === category),
      query,
      aboutPack,
    );
  const asked = query.trim();
  /** The one line a search that finds nothing on this tab shows. */
  const none = (text: string) => (
    <p className="lib-none" role="status">
      {text}
    </p>
  );
  const row = (asset: Asset, art: 'music' | 'sound', add: () => void) => (
    <AssetRow
      key={asset.id}
      asset={asset}
      art={art}
      title={asset.name}
      used={used.has(asset.id)}
      onAdd={add}
      addLabel={art === 'music' ? musicAddLabel(t, asset.name) : undefined}
      onDrop={(target) => addMedia(asset.id, target)}
    />
  );

  if (tab === 'clips') {
    if (own.clips.length === 0)
      return (
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
    const clips = searchLibrary(own.clips, query, aboutOwn('video', 'Your clips'));
    if (clips.length === 0)
      return none(t({ id: 'library.clips.noMatch', message: `No clips match “${asked}”.`, comment: '{asked} is what the person searched for' }));
    return (
      <ul className="lib-list" aria-label={t({ id: 'library.clips', message: 'Your clips' })}>
        {clips.map((clip) => (
          <AssetRow
            key={clip.id}
            asset={clip}
            art="clip"
            title={clip.name}
            used={used.has(clip.id)}
            onAdd={() => addMedia(clip.id, { lane: 'main', index: project.timeline.main.length })}
            addLabel={t({ id: 'library.add.end', message: `Add ${clip.name} to the end`, comment: '{clip.name} is a file name' })}
            onDrop={(target) => addMedia(clip.id, target)}
            menu={<AssetMenu asset={clip} />}
          />
        ))}
      </ul>
    );
  }

  if (tab === 'music' || tab === 'sounds') {
    const music = tab === 'music';
    const mine = searchLibrary(
      music ? own.music : own.sounds,
      query,
      aboutOwn(music ? 'music' : 'sound', music ? 'Your music' : 'Your sounds'),
    );
    const packed = ofPack(music ? 'music' : 'sound');
    return (
      <>
        {mine.length > 0 && (
          <>
            <Group
              id={`lib-${tab}-yours`}
              title={music ? t({ id: 'library.music.yours', message: 'Your music' }) : t({ id: 'library.sounds.yours', message: 'Your sounds' })}
            />
            <ul className="lib-list" aria-labelledby={`lib-${tab}-yours`}>
              {mine.map((asset) =>
                row(asset, music ? 'music' : 'sound', () =>
                  void (music ? addMusic : addSound)({ type: 'asset', id: asset.id }),
                ),
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
      onAdd={() => void addImage({ type: 'asset', id: asset.id })}
      onDrop={(target) => addMedia(asset.id, target)}
    />
  );
  const images = searchLibrary(own.images, query, aboutOwn('photo', 'Your images'));
  const made = searchLibrary(own.made, query, aboutOwn('photo', 'Made with Muse Image'));
  const stickers = ofPack('sticker');
  return (
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
