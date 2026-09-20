import test from 'node:test';
import assert from 'node:assert/strict';
import { durationMonitor, rateLimitedTask, validDuration, safePosition, MAX_MEDIA_SECONDS } from '../android/src/lib/playback.ts';

test('rejects invalid native durations and clamps positions before publishing', () => {
  for (const value of [NaN, Infinity, -Infinity, -1, 0, Number.MAX_VALUE, MAX_MEDIA_SECONDS + 1]) {
    assert.equal(validDuration(value), 0);
  }
  assert.equal(validDuration(3600), 3600);
  assert.equal(safePosition(NaN), 0);
  assert.equal(safePosition(Infinity), 0);
  assert.equal(safePosition(-1), 0);
  assert.equal(safePosition(120, 60), 60);
});

test('native updates coalesce, have no immediate re-entry and retain the final change', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'] });
  let value = 0;
  const sent = [];
  const task = rateLimitedTask(() => { sent.push(value); task.request(); });
  for (value = 0; value < 10000; value++) task.request();
  assert.deepEqual(sent, []);
  t.mock.timers.tick(0);
  assert.deepEqual(sent, [10000]);
  value = 123;
  for (let i = 0; i < 10000; i++) task.request();
  t.mock.timers.tick(999);
  assert.equal(sent.length, 1);
  t.mock.timers.tick(1);
  assert.deepEqual(sent, [10000, 123]);
  task.cancel();
  t.mock.timers.tick(10000);
  assert.equal(sent.length, 2);
});

test('unknown duration gets eight automatic attempts and then stops', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'] });
  let reads = 0;
  const monitor = durationMonitor(() => {});
  monitor.start(() => { reads++; return NaN; });
  for (let i = 0; i < 60; i++) t.mock.timers.tick(1000);
  assert.equal(reads, 8);
  monitor.stop();
});

test('recovers duration without an event, stops polling when found, and resets for a new source', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'] });
  const values = [];
  let duration = NaN;
  let oldReads = 0;
  const monitor = durationMonitor((value) => values.push(value));
  monitor.start(() => { oldReads++; return duration; });
  t.mock.timers.tick(0);
  duration = 60;
  t.mock.timers.tick(1000);
  t.mock.timers.tick(1000);
  assert.equal(values.at(-1), 60);
  const readsWhenFound = oldReads;
  for (let i = 0; i < 10; i++) t.mock.timers.tick(1000);
  assert.equal(oldReads, readsWhenFound);
  monitor.request();
  monitor.start(() => 120);
  t.mock.timers.tick(1000);
  assert.equal(values.at(-1), 120);
  assert.equal(oldReads, readsWhenFound);
  monitor.stop();
});

test('even a duration getter emitting events cannot create an unbounded feedback loop', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout', 'Date'] });
  let reads = 0;
  const monitor = durationMonitor(() => {});
  monitor.start(() => {
    reads++;
    monitor.request();
    return reads;
  });
  for (let second = 0; second < 100; second++) {
    for (let event = 0; event < 1000; event++) monitor.request();
    const before = reads;
    t.mock.timers.tick(1000);
    assert.ok(reads - before <= 1);
  }
  assert.equal(reads, 32);
  monitor.stop();
  monitor.request();
  t.mock.timers.tick(1000);
  assert.equal(reads, 32);
});
