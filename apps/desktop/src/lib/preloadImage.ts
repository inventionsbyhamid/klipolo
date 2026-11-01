/**
 * Resolves once the picture at `url` has loaded and is decoded, so a style that switches to it paints it at once, never
 * an empty frame first. Rejects when it can't be loaded.
 */
export function preloadImage(url: string): Promise<void> {
  const image = new Image();
  const loaded = new Promise<void>((resolve, reject) => {
    image.onload = () => resolve();
    image.onerror = () => reject(new Error(`the picture didn't load: ${url}`));
  });
  image.src = url;
  return loaded.then(() => (typeof image.decode === 'function' ? image.decode() : undefined));
}
