import { describe, it, expect } from 'vitest';
import { externalUrl } from './links';

describe('externalUrl', () => {
  it('accepts web links', () => {
    expect(externalUrl('https://example.com/docs')).toBe('https://example.com/docs');
    expect(externalUrl('http://example.com')).toBe('http://example.com/');
    // Query strings and fragments are part of the link, not decoration.
    expect(externalUrl('https://example.com/a?b=c#d')).toBe('https://example.com/a?b=c#d');
    // A port is still a web link.
    expect(externalUrl('http://localhost:8080/x')).toBe('http://localhost:8080/x');
  });

  it('refuses javascript, which the browser would execute', () => {
    // The reason this is an allowlist. A browser handed one of these runs it, and
    // the text it arrived in is model output.
    expect(externalUrl('javascript:alert(1)')).toBeNull();
    // Case and whitespace are the same scheme to a browser.
    expect(externalUrl('JaVaScRiPt:alert(1)')).toBeNull();
    expect(externalUrl('  javascript:alert(1)')).toBeNull();
    // A newline inside the scheme is stripped by the URL parser, so this would
    // otherwise slip past a check that only compared the prefix.
    expect(externalUrl('java\nscript:alert(1)')).toBeNull();
  });

  it('refuses schemes that reach the machine rather than a page', () => {
    // `file:` would read local files and `data:` is a document in disguise; neither
    // is something to hand a browser on a model's say-so.
    expect(externalUrl('file:///etc/passwd')).toBeNull();
    expect(externalUrl('data:text/html,<script>alert(1)</script>')).toBeNull();
    expect(externalUrl('vbscript:msgbox(1)')).toBeNull();
  });

  it('refuses what is not a link out', () => {
    expect(externalUrl(undefined)).toBeNull();
    expect(externalUrl('')).toBeNull();
    // Relative to the app's own document, which is not somewhere a browser can go.
    expect(externalUrl('/docs')).toBeNull();
    expect(externalUrl('docs/page')).toBeNull();
    expect(externalUrl('#section')).toBeNull();
  });

  it('sees through the tricks a URL parser normalises away', () => {
    // Each of these is a `javascript:` URL that a browser would execute, and each
    // is rejected here because the parser reports the scheme the browser acts on —
    // not the one that was written. Compared by name against the raw string, every
    // one of them would have slipped through.
    expect(externalUrl('java\nscript:alert(1)')).toBeNull();
    expect(externalUrl('java\tscript:alert(1)')).toBeNull();
    expect(externalUrl('\u0001javascript:alert(1)')).toBeNull();
  });

  it('normalises scheme case rather than refusing it', () => {
    // `HTTPS:` is not a different protocol from `https:`, and refusing it would
    // break a link that works everywhere else.
    expect(externalUrl('HTTPS://EXAMPLE.COM')).toBe('https://example.com/');
  });

  it('returns the parsed href, so what opens is what was checked', () => {
    // Callers must not be handed the original string back: it is the parsed form
    // that has been through the allowlist.
    expect(externalUrl('https://example.com')).toBe('https://example.com/');
    expect(externalUrl('https://example.com/a b')).toBe('https://example.com/a%20b');
  });

  it('keeps credentials and ports intact rather than mangling the link', () => {
    // Some internal docs run on a host with a port; a stripped link is a dead link.
    expect(externalUrl('https://user:pw@internal.host:8443/x')).toBe(
      'https://user:pw@internal.host:8443/x',
    );
  });
});