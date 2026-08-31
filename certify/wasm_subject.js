/* The browser build, made a subject the sweeps can drive.

   The certification machinery drives a *process*: stanzas in on
   stdin, stanzas out on stdout, one line each. A wasm module is
   not a process, so this is the thinnest possible shim that makes
   it look like one -- load the module, hand it what arrives, print
   what it answers. Nothing here interprets a stanza or knows what
   one means; if it did, it would be certifying itself.

   With that, `certify/wasm-diff.sh` can drive the browser build
   through the very typist that drives the stdio subject, and diff
   the two transcripts byte for byte.

   Usage:
     node certify/wasm_subject.js <glue.js> <story-file> [seed]
*/

"use strict";

const fs = require("fs");
const path = require("path");
const readline = require("readline");

async function main() {
  const [gluePath, storyPath, seedText] = process.argv.slice(2);

  if (!gluePath || !storyPath) {
    console.error("usage: wasm_subject.js <glue.js> <story-file> [seed]");
    process.exit(2);
  }

  const wasm = require(path.resolve(gluePath));
  const bytes = fs.readFileSync(storyPath);

  /* The sidecar a bare story keeps beside it, as every other host
     finds it -- the module itself has no filesystem to look in. */
  const BLORBS = [".blb", ".blorb", ".zblorb", ".gblorb"];
  const suffix = path.extname(storyPath).toLowerCase();
  let resources;

  if (!BLORBS.includes(suffix)) {
    for (const beside of BLORBS) {
      const sidecar = storyPath.slice(0, -suffix.length || undefined) + beside;

      if (fs.existsSync(sidecar)) {
        resources = new Uint8Array(fs.readFileSync(sidecar));
        break;
      }
    }
  }

  let story;

  try {
    story = wasm.Story.open(new Uint8Array(bytes), {
      name: path.basename(storyPath),
      resources,
      seed: seedText === undefined ? undefined : Number(seedText),
    });
  } catch (refusal) {
    /* A story that will not load is the module's one throw, and it
       reads as the CLI's own pre-wire refusal. */
    console.log(String(refusal.message || refusal));
    process.exit(2);
  }

  /* Answers arrive a microtask late, so the drive is a queue: each
     line in, its answer out, in the order they were asked. */
  const waiting = [];

  story.onStanza((text) => {
    const settle = waiting.shift();

    console.log(text);

    if (settle) settle();
  });

  const lines = readline.createInterface({ input: process.stdin });

  for await (const line of lines) {
    if (!line.trim()) continue;

    const answered = new Promise((settle) => waiting.push(settle));

    story.send(line);
    await answered;
  }

  story.free();
}

main().catch((fault) => {
  console.error(String(fault && fault.stack ? fault.stack : fault));
  process.exit(2);
});
