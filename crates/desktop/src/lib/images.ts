import type { Attachment } from '@/lib/types';

// The last data URL built for each attachment. The string is the image itself
// in base64 — megabytes for a screenshot — and the components that draw an
// image re-render far more often than the image changes: the turn being
// streamed re-renders on every chunk, the composer on every keystroke. Built
// fresh each time, that is a multi-megabyte copy per frame; the attachment
// objects are replaced only when the image itself is, so the key holds.
const CACHE = new WeakMap<Attachment, string>();

/**
 * The data URL an attachment is shown from.
 *
 * The transcript, the composer and the preview all draw the same bytes, so the
 * header is spelled in one place: `data` is bare base64, and it is the
 * `;base64` half of the mime type that tells the browser so rather than
 * reading it as text.
 */
export function imageSrc(image: Attachment): string {
  let src = CACHE.get(image);
  if (src === undefined) {
    src = `data:${image.mime};base64,${image.data}`;
    CACHE.set(image, src);
  }
  return src;
}
