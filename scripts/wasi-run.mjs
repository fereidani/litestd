// Runs a WASI preview 1 program under Node's WASI, as Cargo's runner:
//
//   CARGO_TARGET_WASM32_WASIP1_RUNNER="node --no-warnings scripts/wasi-run.mjs"
//
// and the same for `wasm32-wasip1-threads`. It needs Node 24 or later: Node
// 22 crashes when a garbage collection runs inside its `path_open`.
//
// The program sees a new scratch directory as `/`, with `/tmp` inside it,
// and nothing else of the file system; the directory is removed when the
// program ends. It gets the arguments and the environment of this process,
// and exits with the program's status, or 1 if the program traps.
//
// A program built for wasi-threads, which imports a shared memory and
// `wasi.thread-spawn`, runs each of its threads in a worker with a WASI
// instance of its own, so a descriptor that one thread opens is unknown to
// the others. This thread only starts the workers, so it never blocks, and
// ends the program as soon as one of them exits or traps. Workers sleep in
// `Atomics.wait`, which the exit interrupts, unlike Node's `poll_oneoff`.

import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { argv, env, exit } from 'node:process';
import { WASI } from 'node:wasi';
import {
  isMainThread,
  parentPort,
  Worker,
  workerData,
} from 'node:worker_threads';

/** Thrown by `proc_exit` in a worker, to leave the module. */
const EXIT = Symbol('exit');

/** The sizes of WASI's `subscription` and `event`. */
const SUBSCRIPTION = 48;
const EVENT = 32;

/**
 * Wraps `poll`, WASI's `poll_oneoff`, so that a poll on relative clocks
 * alone sleeps in `Atomics.wait` and reports the earliest clock.
 */
function interruptiblePoll(memory, poll) {
  const sleeper = new Int32Array(new SharedArrayBuffer(4));
  return (subscriptions, events, count, eventCount) => {
    const view = new DataView(memory.buffer);
    let earliest = -1;
    let shortest = Infinity;
    for (let i = 0; i < count; i++) {
      const at = subscriptions + i * SUBSCRIPTION;
      // A tag other than 0 is not a clock; flag 1 makes the time absolute.
      if (view.getUint8(at + 8) !== 0 || view.getUint16(at + 40, true) & 1) {
        return poll(subscriptions, events, count, eventCount);
      }
      const timeout = Number(view.getBigUint64(at + 24, true));
      if (timeout < shortest) {
        earliest = i;
        shortest = timeout;
      }
    }
    if (earliest < 0) return poll(subscriptions, events, count, eventCount);
    Atomics.wait(sleeper, 0, 0, shortest / 1e6);
    const userdata = subscriptions + earliest * SUBSCRIPTION;
    new Uint8Array(memory.buffer, events, EVENT).fill(0);
    view.setBigUint64(events, view.getBigUint64(userdata, true), true);
    view.setUint32(eventCount, 1, true);
    return 0;
  };
}

function wasiFor(args, root) {
  return new WASI({
    version: 'preview1',
    args,
    env,
    preopens: { '/': root },
    returnOnExit: true,
  });
}

/**
 * Returns the shared memory that the module in `bytes` imports, with the
 * limits it declares, or `undefined` if it imports none.
 */
function sharedMemory(bytes) {
  let at = 8;
  const leb = () => {
    let value = 0;
    for (let shift = 0; ; shift += 7) {
      const byte = bytes[at++];
      value += (byte & 0x7f) * 2 ** shift;
      if (byte < 0x80) return value;
    }
  };
  // `leb` moves `at`, so its result is added in a statement of its own.
  const skipName = () => {
    const length = leb();
    at += length;
  };
  const skipLimits = () => {
    const flags = bytes[at++];
    leb();
    if (flags & 1) leb();
  };
  while (at < bytes.length) {
    const id = bytes[at++];
    const size = leb();
    const end = at + size;
    if (id === 2) {
      for (let count = leb(); count > 0; count--) {
        skipName();
        skipName();
        const kind = bytes[at++];
        if (kind === 0) leb();
        else if (kind === 1) at++, skipLimits();
        else if (kind === 3) at += 2;
        else if (kind === 4) at++, leb();
        else {
          const flags = bytes[at++];
          const initial = leb();
          const maximum = flags & 1 ? leb() : undefined;
          return flags & 2
            ? new WebAssembly.Memory({ initial, maximum, shared: true })
            : undefined;
        }
      }
      return undefined;
    }
    at = end;
  }
  return undefined;
}

if (isMainThread) {
  const [, , file, ...args] = argv;
  const root = mkdtempSync(join(tmpdir(), 'litestd-wasi-'));
  mkdirSync(join(root, 'tmp'));
  const bytes = readFileSync(file);
  const module = await WebAssembly.compile(bytes);
  const memory = sharedMemory(bytes);
  if (memory === undefined) {
    let status = 1;
    try {
      const wasi = wasiFor([file, ...args], root);
      const instance = await WebAssembly.instantiate(
        module,
        wasi.getImportObject(),
      );
      status = wasi.start(instance);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
    exit(status);
  }
  const shared = {
    module,
    memory,
    args: [file, ...args],
    root,
    lastId: new Int32Array(new SharedArrayBuffer(4)),
  };
  const end = (status) => {
    rmSync(root, { recursive: true, force: true });
    exit(status);
  };
  const start = (id, arg) => {
    const worker = new Worker(new URL(import.meta.url), {
      workerData: { ...shared, id, arg },
    });
    worker.on('message', (message) =>
      message.spawn ? start(...message.spawn) : end(message.exit),
    );
    worker.on('error', (error) => {
      console.error(error);
      end(1);
    });
  };
  start(0, 0);
} else {
  const { module, memory, args, root, lastId, id, arg } = workerData;
  const wasi = wasiFor(args, root);
  const imports = wasi.getImportObject();
  const preview1 = imports.wasi_snapshot_preview1;
  preview1.proc_exit = (status) => {
    parentPort.postMessage({ exit: status });
    throw EXIT;
  };
  preview1.poll_oneoff = interruptiblePoll(memory, preview1.poll_oneoff);
  imports.env = { memory };
  imports.wasi = {
    'thread-spawn': (startArg) => {
      const newId = Atomics.add(lastId, 0, 1) + 1;
      parentPort.postMessage({ spawn: [newId, startArg] });
      return newId;
    },
  };
  const instance = new WebAssembly.Instance(module, imports);
  try {
    if (id === 0) {
      wasi.start(instance);
      parentPort.postMessage({ exit: 0 });
    } else {
      wasi.initialize({ exports: { memory } });
      instance.exports.wasi_thread_start(id, arg);
    }
  } catch (error) {
    if (error !== EXIT) throw error;
  }
}
