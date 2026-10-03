/**
 * Where a link in assistant output should go.
 *
 * Assistant text is model output, so a link in it is untrusted input like any other:
 * this decides whether it is passed to the user's browser at all, and a browser
 * handed a `javascript:` URL will run it.
 */

/**
 * The URL a link should open, or `null` if it should not open.
 *
 * An allowlist rather than a blocklist. `javascript:` is the reason that matters,
 * and blocking it by name would mean guessing every spelling of it — mixed case,
 * embedded whitespace, entity-encoded — where an allowlist only has to be right
 * once. `data:` and `file:` fall away for the same reason.
 *
 * A relative href returns `null` too. There is nothing sensible to hand the browser
 * for one, and treating it as a path out of the app is not what a reader means by
 * clicking it.
 *
 * Returns the parsed href rather than the input, so what gets opened is what was
 * checked — a caller cannot pass the original string off unchecked.
 */
export function externalUrl(href: string | undefined): string | null {
  if (!href) return null;

  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return null;
  }

  return url.protocol === 'http:' || url.protocol === 'https:' ? url.href : null;
}

/**
 * Opens a URL in the user's own browser rather than in the app's webview.
 *
 * A click on a link would otherwise navigate the webview to that URL, replacing the
 * whole application with a web page.
 */
export async function openExternal(url: string): Promise<void> {
  const { openUrl } = await import('@tauri-apps/plugin-opener');
  await openUrl(url);
}