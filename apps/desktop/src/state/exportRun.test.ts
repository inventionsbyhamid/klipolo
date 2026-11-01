import { describe, expect, it } from 'vitest';
import type { ExportEvent } from '../bindings/ExportEvent';
import type { ExportJobInfo } from '../bindings/ExportJobInfo';
import {
  applyEvent,
  canStartOver,
  exportPercent,
  idleRun,
  resuming,
  shownJob,
  unfinishedJobs,
  unsavedFormats,
  type ExportRun,
} from './exportRun';

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

  it('a run that stops keeping its parts is paused, calmly, saying why, and its job is kept', () => {
    const diskFull = { code: 'exports.diskFull', params: {} };
    const s = run([started, progress(80, 12), { type: 'paused', message: diskFull, compatibility: null }]);
    const why = 'The disk is full. Free up space, then resume.';
    expect([s.phase, s.jobId, s.message, s.etaS]).toEqual(['paused', 'job', why, null]);
    expect(s.announcement).toBe(`Export paused. ${why}`);
    expect(exportPercent(s)).toBe(40);
    // The person's pause (Pause, Quit) says no reason.
    const own = run([started, { type: 'paused', message: null, compatibility: null }]);
    expect([own.phase, own.message, own.announcement]).toEqual(['paused', null, 'Export paused.']);
  });

  it('a resume shows the job as it was at once, from the list or from the screen', () => {
    const listed: ExportJobInfo = {
      jobId: 'kept',
      projectId: 'p',
      projectName: 'Demo',
      quality: 'high',
      files: [file],
      state: 'interrupted',
      message: null,
      frame: 120,
      totalFrames: 200,
      segmentsDone: 2,
      segmentsTotal: 4,
      stale: false,
      createdMs: 0,
      bytes: 0,
      compatibility: null,
    };
    const fromList = { ...idleRun, ...resuming(idleRun, 'kept', listed) };
    expect([fromList.phase, fromList.jobId, exportPercent(fromList), fromList.segmentsDone]).toEqual([
      'running',
      'kept',
      60,
      2,
    ]);
    expect(fromList.announcement).toBe('Export resumed.');
    // Paused on screen with a file in place: it stays listed as the job goes on.
    const paused = run([
      { ...started, files: [file, portrait] } as ExportEvent,
      progress(100, 3),
      { type: 'fileReady', index: 0, path: file.path },
      { type: 'paused', message: null, compatibility: null },
    ]);
    const again = { ...paused, ...resuming(paused, 'job') };
    expect([again.phase, again.ready, exportPercent(again)]).toEqual(['running', [0], 50]);
    // The job on screen isn't among the unfinished rows; a job none of whose formats is left can't start over.
    const view = { jobId: 'kept', jobs: [listed, { ...listed, jobId: 'other' }] };
    expect(shownJob(view)).toBe(listed);
    expect(unfinishedJobs(view).map((j) => j.jobId)).toEqual(['other']);
    expect(canStartOver(listed, ['landscape'])).toBe(true);
    expect(canStartOver(listed, ['portrait'])).toBe(false);
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
    expect(s.message).toBe('The disk is full. Free up space, then resume.');
    expect(unsavedFormats(s)).toEqual(['portrait']);
    // What was lost first, then why and what to do.
    expect(s.announcement).toBe('9:16 was not saved. The disk is full. Free up space, then resume.');
    expect(s.ready).toEqual([0]);
  });

  it('progress is said at each quarter, never every frame', () => {
    let s = run([started, progress(10, null)]);
    expect(s.announcement).toBe('');
    s = { ...s, ...applyEvent(s, progress(50, 20)) };
    expect(s.announcement).toBe('25% exported.');
    s = { ...s, ...applyEvent(s, progress(60, 18)) };
    expect(s.announcement).toBe('25% exported.');
    s = { ...s, ...applyEvent(s, progress(160, 4)) };
    expect(s.announcement).toBe('75% exported.');
  });
});
