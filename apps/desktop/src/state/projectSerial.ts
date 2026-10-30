let queue: Promise<unknown> = Promise.resolve();

/** Project commands run strictly one after another so edits never race. */
export function serial<T>(task: () => Promise<T>): Promise<T> {
  const run = queue.then(task, task);
  queue = run.catch(() => undefined);
  return run;
}

/** Resolves once every project command queued so far has finished (export reads the saved project). */
export function settled(): Promise<void> {
  return queue.then(() => undefined);
}
