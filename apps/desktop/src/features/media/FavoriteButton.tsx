import { useLingui } from '@lingui/react/macro';
import { cn } from '../../design/cn';
import { IconButton } from '../../design/IconButton';
import { SpriteIcon } from '../../design/SpriteIcon';

/** Whether an item is a favorite, and how to change that. */
export interface Favorite {
  on: boolean;
  toggle: () => void;
}

/**
 * A library item's star (a toggle named "Favorite <title>"): it appears on hover or focus with the "+", and stays,
 * filled, while the item is a favorite.
 */
export function FavoriteButton({ title, favorite }: { title: string; favorite: Favorite }) {
  const { t } = useLingui();
  return (
    <IconButton
      icon={<SpriteIcon name="star" />}
      label={t({
        id: 'library.favorite',
        message: `Favorite ${title}`,
        comment: 'A toggle on a library row; {title} is a file or a sound’s name',
      })}
      tooltip={
        favorite.on
          ? t({ id: 'library.favorite.remove', message: 'Remove from favorites' })
          : t({ id: 'library.favorite.add', message: 'Add to favorites' })
      }
      active={favorite.on}
      className={cn('row-action', 'favorite')}
      onClick={favorite.toggle}
    />
  );
}
