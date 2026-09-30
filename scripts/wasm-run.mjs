// Runs a wasm32-unknown-unknown program under Node, as Cargo's runner:
//
//   CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER="node scripts/wasm-run.mjs"
//
// The module has no OS: it imports nothing, and what it prints goes
// nowhere. This calls its exported `main(0, 0)` and exits with the result.
// A panic ends in a trap, whose stack, printed here, names the failing test;
// the exit status is then 1.

import { readFileSync } from 'node:fs';
import { argv, exit } from 'node:process';

const { instance } = await WebAssembly.instantiate(readFileSync(argv[2]), {});
try {
  exit(instance.exports.main(0, 0));
} catch (error) {
  console.error(error.stack);
  exit(1);
}
