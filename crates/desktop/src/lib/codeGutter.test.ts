// Checks the code gutter against a real browser.
//
// This exists because the gap between the line number and the code was wrong
// twice, and both times the mistake was invisible in the markup: `padding-left`
// moves the gutter's left edge and does nothing at all between the number and the
// first character, so the CSS read as though a gap were specified while the
// rendered result had none. Nothing short of measuring the box catches that.
//
// `playwright-core` does not download a browser, so this skips when none is
// installed rather than failing. The rest of the suite is unaffected: it runs in
// the default node environment and never launches anything.
import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { homedir } from 'node:os';
import type { Browser } from 'playwright-core';

/** The rules this test is about, lifted from the stylesheet so it cannot drift. */
const RULES = [
  '.code-block',
  '.code-block pre',
  '.code-block pre code',
  '.code-block pre code .code-line',
  '.code-line::before',
];

/** Pulls one rule out of the stylesheet, braces and all. */
function rule(source: string, selector: string): string {
  const at = source.indexOf(selector);
  if (at < 0) throw new Error(`no rule for ${selector}`);
  let depth = 0;
  for (let i = at; i < source.length; i++) {
    if (source[i] === '{') depth++;
    else if (source[i] === '}' && --depth === 0) return source.slice(at, i + 1);
  }
  throw new Error(`unterminated rule for ${selector}`);
}

/** A chromium from playwright's cache, or null when there is not one. */
function findChromium(): string | null {
  const cache = join(homedir(), '.cache', 'ms-playwright');
  if (!existsSync(cache)) return null;
  const builds = readdirSync(cache).filter((name: string) => name.startsWith('chromium-'));
  for (const build of builds) {
    for (const relative of ['chrome-linux64/chrome', 'chrome-linux/chrome']) {
      const path = join(cache, build, relative);
      if (existsSync(path)) return path;
    }
  }
  return null;
}

const executablePath = findChromium();

describe.skipIf(!executablePath)('code gutter geometry', () => {
  let browser: Browser;

  beforeAll(async () => {
    const { chromium } = await import('playwright-core');
    browser = await chromium.launch({ executablePath: executablePath! });
  });

  afterAll(async () => {
    await browser?.close();
  });

  /** Renders lines in a real browser and reports where the code actually starts. */
  async function measure(): Promise<{
    gutterPx: number;
    gapPx: number;
    perLine: (number | null)[];
    numberIsOutOfFlow: boolean;
  }> {
    const css = readFileSync(new URL('../styles.css', import.meta.url), 'utf8');
    const page = await browser.newPage();
    await page.setContent(`<!doctype html><html><head><style>
      * { box-sizing: border-box; }
      body { margin: 0; font-family: system-ui, sans-serif; }
      ${RULES.map((selector) => rule(css, selector)).join('\n')}
    </style></head><body>
      <div class="code-block"><pre><code
        ><span class="code-line">first</span
        ><span class="code-line">second</span
        ><span class="code-line">third</span
      ></code></pre></div>
    </body></html>`);

    const result = await page.evaluate(() => {
      const line = document.querySelector('.code-line')!;
      const style = getComputedStyle(line);
      const before = getComputedStyle(line, '::before');
      const em = (value: string) => parseFloat(value) * parseFloat(style.fontSize);

      // The first glyph, rather than the content box: the box starts where the
      // padding ends whether or not anything is visible there.
      const glyphOffset = (el: Element) => {
        const range = document.createRange();
        const target = el.firstChild ?? el;
        range.setStart(target, 0);
        range.setEnd(target, 0);
        const rects = range.getClientRects();
        return rects.length ? rects[0].left - el.getBoundingClientRect().left : null;
      };

      return {
        gutterPx: em(style.getPropertyValue('--code-gutter')),
        gapPx: em(style.getPropertyValue('--code-gutter-gap')),
        // An in-flow number sits in its own box before the text; out of flow it
        // is positioned, which is what stops a long line pushing it along.
        numberIsOutOfFlow: before.position === 'absolute',
        perLine: [...document.querySelectorAll('.code-line')].map(glyphOffset),
      };
    });

    await page.close();
    return result;
  }

  it('leaves a real gap between the number and the code', async () => {
    // The regression this file is for. The gap has to be a stated value, not the
    // remainder of `padding - margin - width`, which is how it read as present
    // while rendering as zero.
    const { gapPx } = await measure();

    expect(gapPx).toBeGreaterThan(4);
  });

  it('starts the code at the same place on every line', async () => {
    const { gutterPx, perLine } = await measure();

    expect(perLine).toEqual(perLine.map(() => gutterPx));
  });

  it('takes the number out of flow', async () => {
    // In flow, a long line would push the number and the code along together and
    // the gutter would grow with the content.
    const { numberIsOutOfFlow } = await measure();

    expect(numberIsOutOfFlow).toBe(true);
  });
});
