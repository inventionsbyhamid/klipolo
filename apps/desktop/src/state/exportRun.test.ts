import { describe, expect, it } from 'vitest';
import type { ExportEvent } from '../bindings/ExportEvent';
import { applyEvent, exportPercent, idleRun, unsavedFormats, type ExportRun } from './exportRun';

const file = {
  format: 'landscape' as const,
  width: 1920,
  height: 1080,
  name: 'Demo 16x9.mp4',
  path: '/x/Demo 16x9.mp4',
};
const portrait = { ...file, format: 'portrait' as const, width: 1080, height: 1920, name: 'Demo 9x16.mp4' };

function run(events: ExportEvent[]): ExportRun {
  return events.reduce<ExportRun>((s, e) => ({ ...s, ...applyEvent(s, e) }), {
    ...idleRun,
    phase: 'running',
  });
}

const started: ExportEvent = {
  type: 'started',
  jobId: 'job',
  files: [file],
  totalFrames: 200,
  segmentsTotal: 4,
};
const progress = (frame: number, etaS: number | null): ExportEvent => ({
  type: 'progress',
  frame,
  totalFrames: 200,
  etaS,
  segmentsDone: Math.floor(frame / 50),
  segmentsTotal: 4,
});

describe('export run', () => {
  it('finishing never shows 100 % or a time left', () => {
    const s = run([started, progress(200, 0.4), { type: 'finishing' }, progress(200, 0)]);
    expect(s.finishing).toBe(true);
    expect(s.etaS).toBeNull();
    expect(exportPercent(s)).toBe(99);
    expect(s.segmentsDone).toBe(4);
  });

  it('the time left returns after a retry, and compatibility mode stays noted', () => {
    let s = run([started, progress(40, 30), { type: 'retrying' }, progress(50, null)]);
    expect(s.retrying).toBe(true);
    s = { ...s, ...applyEvent(s, progress(60, 25)) };
    expect([s.retrying, s.etaS]).toEqual([false, 25]);
    s = run([started, { type: 'retrying' }, progress(50, 20), { type: 'retrying' }, progress(55, 90)]);
    expect([s.retries, s.retrying, s.etaS]).toEqual([2, false, 90]);
  });

  it('a run that stops keeping its parts has failed here, and says why', () => {
    // Klipolo doesn't resume an export: Rust says a stop as a failure, but a pause that reaches the page is one too.
    const message = 'Your disk is full. Free up space, then resume.';
    const s = run([started, progress(80, 12), { type: 'paused', message, compatibility: null }]);
    expect([s.phase, s.jobId, s.message, s.etaS]).toEqual(['failed', 'job', message, null]);
    expect(s.announcement).toBe(`${message} 16:9 was not saved.`);
  });

  it('rows show the file name as written', () => {
    const s = run([
      started,
      { type: 'fileReady', index: 0, path: '/x/Demo 16x9 2.mp4' },
      { type: 'finished' },
    ]);
    expect(s.files[0].name).toBe('Demo 16x9 2.mp4');
    expect([s.phase, s.ready, s.announcement]).toEqual(['done', [0], 'Exported 1 video']);
  });

  it('a failure names the formats that were not saved, and keeps the ones that were', () => {
    const two: ExportEvent = { ...started, files: [file, portrait] } as ExportEvent;
    const s = run([
      two,
      progress(200, 0),
      { type: 'fileReady', index: 0, path: file.path },
      { type: 'failed', message: { code: 'exports.diskFull', params: {} } },
    ]);
    expect(s.phase).toBe('failed');
    expect(s.message).toBe('The disk is full.');
    expect(unsavedFormats(s)).toEqual(['portrait']);
    expect(s.announcement).toBe('The disk is full. 9:16 was not saved.');
    expect(s.ready).toEqual([0]);
  });
});
