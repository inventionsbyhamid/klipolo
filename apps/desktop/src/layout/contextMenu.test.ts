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
});
