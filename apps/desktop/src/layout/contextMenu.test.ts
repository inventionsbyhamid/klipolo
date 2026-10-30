import { afterEach, describe, expect, it } from 'vitest';
import { suppressContextMenu } from './contextMenu';

afterEach(() => document.body.replaceChildren());

function rightClick(target: Element): boolean {
  const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event.defaultPrevented;
}

describe('suppressContextMenu', () => {
  it("hides the webview's menu (Reload, Inspect) everywhere but text fields", () => {
    const stop = suppressContextMenu(document);
    document.body.innerHTML =
      '<button>Play</button><div><span>Library</span></div><textarea></textarea><input><div contenteditable="true"><b>x</b></div>';
    expect(rightClick(document.querySelector('button')!)).toBe(true);
    expect(rightClick(document.querySelector('span')!)).toBe(true);
    expect(rightClick(document.querySelector('textarea')!)).toBe(false);
    expect(rightClick(document.querySelector('input')!)).toBe(false);
    expect(rightClick(document.querySelector('b')!)).toBe(false);
    stop();
    expect(rightClick(document.querySelector('button')!)).toBe(false);
  });

  it('keeps the menu only where text can be edited: not on a disabled or read-only field, a checkbox or a slider', () => {
    const stop = suppressContextMenu(document);
    document.body.innerHTML = [
      '<input id="text" type="text">',
      '<input id="search" type="search">',
      '<input id="disabled" disabled>',
      '<input id="readonly" readonly>',
      '<textarea id="locked" disabled></textarea>',
      '<input id="check" type="checkbox">',
      '<input id="range" type="range">',
      '<div contenteditable="false"><i id="off">x</i></div>',
    ].join('');
    const at = (id: string) => rightClick(document.getElementById(id)!);
    expect([at('text'), at('search')]).toEqual([false, false]);
    expect([at('disabled'), at('readonly'), at('locked'), at('check'), at('range'), at('off')]).toEqual([
      true,
      true,
      true,
      true,
      true,
      true,
    ]);
    stop();
  });
});
