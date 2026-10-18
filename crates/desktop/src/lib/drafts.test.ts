// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { loadDrafts, saveDrafts } from './drafts';

beforeEach(() => {
  localStorage.clear();
});

describe('loadDrafts', () => {
  it('hands back what saveDrafts wrote', () => {
    saveDrafts({ S1: 'leftover', S2: 'half a thought' });
    expect(loadDrafts()).toEqual({ S1: 'leftover', S2: 'half a thought' });
  });

  it('is empty when nothing was ever written', () => {
    expect(loadDrafts()).toEqual({});
  });

  it('is empty when the stored value is not JSON', () => {
    localStorage.setItem('srud.drafts', '{not json');
    expect(loadDrafts()).toEqual({});
  });

  it('is empty when the stored value is not an object', () => {
    localStorage.setItem('srud.drafts', '"a string"');
    expect(loadDrafts()).toEqual({});
  });

  it('keeps the text entries and drops the rest', () => {
    localStorage.setItem('srud.drafts', JSON.stringify({ S1: 'text', S2: 7, S3: null }));
    expect(loadDrafts()).toEqual({ S1: 'text' });
  });
});

describe('saveDrafts', () => {
  it('writes the map it is given', () => {
    saveDrafts({ S1: 'text' });
    expect(JSON.parse(localStorage.getItem('srud.drafts')!)).toEqual({ S1: 'text' });
  });

  it('leaves the drafts alone when the webview refuses the write', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota');
    });
    expect(() => saveDrafts({ S1: 'text' })).not.toThrow();
  });
});
