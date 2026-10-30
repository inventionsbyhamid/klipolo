import { i18n } from '@lingui/core';
import { msg, plural, select, t } from '@lingui/core/macro';
import { Trans as TransById, useLingui as useI18n } from '@lingui/react';
import { Trans, useLingui } from '@lingui/react/macro';
import { render, renderHook, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { formatMessage } from './format';

describe('the English-only message shim', () => {
  it('fills in named values and leaves unknown ones as they are', () => {
    expect(formatMessage('Add {effect}', { effect: 'Blur' })).toBe('Add Blur');
    expect(formatMessage('Renamed {name}', {})).toBe('Renamed {name}');
    // A file name with braces in it stays as it is.
    expect(formatMessage('a{b}.mov and {c', {})).toBe('a{b}.mov and {c');
  });

  it('picks the branch of a select, and other when nothing matches', () => {
    const message = '{layer, select, text {Move text} image {Move image} other {Move layer}}';
    expect(formatMessage(message, { layer: 'image' })).toBe('Move image');
    expect(formatMessage(message, { layer: 'camera' })).toBe('Move layer');
  });

  it('picks the plural branch by count, exact matches first, with # as the number', () => {
    const message = '{count, plural, =0 {No clips} one {# clip} other {# clips}}';
    expect(formatMessage(message, { count: 0 })).toBe('No clips');
    expect(formatMessage(message, { count: 1 })).toBe('1 clip');
    expect(formatMessage(message, { count: 1200 })).toBe('1,200 clips');
    expect(formatMessage('{n, plural, one {{name} and # more} other {{name} and # more}}', { n: 2, name: 'a' })).toBe(
      'a and 2 more',
    );
  });

  it('a # in a value stays as written; only the # in the branch text is the count', () => {
    const message = '{n, plural, one {{name} and # more} other {{name} and # more}}';
    expect(formatMessage(message, { n: 2, name: 'Take #3.mov' })).toBe('Take #3.mov and 2 more');
    // The plural macro gets its branches already filled in, so it can't tell: callers keep values outside it.
    expect(`Take #3.mov ${plural(2, { one: 'and # more', other: 'and # more' })}`).toBe('Take #3.mov and 2 more');
  });

  it('the macros return English: t, msg, plural and select', () => {
    const name = 'intro.mov';
    expect(t({ id: 'media.added', message: `Added ${name}` })).toBe('Added intro.mov');
    expect(t`Hello ${name}`).toBe('Hello intro.mov');
    expect(plural(1, { one: '# file', other: '# files' })).toBe('1 file');
    expect(plural(3, { one: '# file', other: '# files' })).toBe('3 files');
    expect(plural(1, { 1: 'Skipped one', other: 'Skipped # files' })).toBe('Skipped one');
    expect(select('video', { video: 'Video', other: 'Layer' })).toBe('Video');
    const descriptor = msg({ id: 'undo.cut', message: 'Cut', comment: 'Undo history' });
    expect(descriptor).toMatchObject({ id: 'undo.cut', message: 'Cut' });
    expect(i18n._(descriptor)).toBe('Cut');
    expect(i18n._('effect.add.undo', { effect: 'Blur' }, { message: 'Add {effect}' })).toBe('Add Blur');
    expect(i18n.locale).toBe('en');
    expect(i18n.number(1234.5)).toBe('1,234.5');
  });

  it('Trans renders its children (macro) or its message by id; useLingui gives t and i18n', () => {
    render(
      <p>
        <Trans id="toast.error" comment="Read before an error">
          Error:
        </Trans>{' '}
        <TransById id="lane" message="Lane {n}" values={{ n: 2 }} />
      </p>,
    );
    expect(screen.getByText('Error: Lane 2').tagName).toBe('P');
    const macro = renderHook(() => useLingui()).result.current;
    expect(macro.t({ id: 'x', message: 'Fit' })).toBe('Fit');
    expect(renderHook(() => useI18n()).result.current.i18n.locale).toBe('en');
  });
});
