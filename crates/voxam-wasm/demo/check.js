/* The demo's own drill: the adapter, driven headlessly.

   The page can only be judged by eye, but the wiring inside it
   need not be. This stands a stub GlkOte in front of the real
   module and plays a real story through the very adapter the page
   loads -- so a rename, a dropped callback, or a broken handshake
   is caught here rather than in a browser.

   Usage, after crates/voxam-wasm/build.sh:
     node crates/voxam-wasm/demo/check.js <story-file>
*/

"use strict";

const fs = require("fs");
const path = require("path");

const root = path.resolve(__dirname, "../../..");
const wasm = require(path.join(root, "target/wasm/nodejs/voxam_wasm.js"));
const story = process.argv[2] || path.join(root, "entharion/zcode-infocom/zork1-r88-s840726.z3");

/* What the page provides and the adapter expects to find. */
const seen = [];
let asked = null;

global.GlkOte = {
  update(stanza) {
    seen.push(stanza);
  },
  /* The adapter never calls these, but the page's GlkOte would. */
  init() {
    asked = { type: "init", gen: 0, support: ["timer"],
      metrics: { width: 800, height: 480, gridcharwidth: 10, gridcharheight: 20 } };
    global.Game.accept(asked);
  },
};
global.window = undefined;

const { voxamGame } = require("./voxam-glkote.js");

let failures = 0;

function check(name, held, want) {
  const ok = held === want;

  if (!ok) failures++;

  console.log(`${ok ? "ok  " : "FAIL"}  ${name}: ${held} (wanted ${want})`);
}

(async () => {
  const opened = wasm.Story.open(new Uint8Array(fs.readFileSync(story)), {
    name: path.basename(story),
    seed: 1,
  });

  global.Game = voxamGame(opened);

  check("the adapter offers an accept", typeof global.Game.accept, "function");
  check("and a Dialog for the file prompt", typeof global.Game.Dialog.open, "function");

  /* A host with nowhere to save cancels, and says so by handing
     back nothing at all. */
  let answered = "unset";

  global.Game.Dialog.open(true, "save", "game", (name) => {
    answered = name;
  });
  check("with no prompt hook it cancels", answered, null);

  /* The whole handshake: GlkOte opens with its init, the story
     answers a microtask later, and the answer lands in update. */
  GlkOte.init();
  check("nothing arrives synchronously", seen.length, 0);

  await new Promise((settle) => setTimeout(settle, 0));

  check("the init is answered", seen.length, 1);
  check("and it is an update", seen[0] && seen[0].type, "update");

  const opening = JSON.stringify(seen[0]);

  check("the story is really running", opening.includes("ZORK I"), true);

  /* One typed turn, to prove the loop and not just the opening. */
  global.Game.accept({ type: "line", gen: seen[0].gen, window: 1, value: "open mailbox" });
  await new Promise((settle) => setTimeout(settle, 0));

  check("a typed turn is answered", seen.length, 2);
  check("and the story acted on it", JSON.stringify(seen[1]).includes("small mailbox"), true);

  opened.free();

  console.log(failures ? `\n${failures} failed` : "\nthe adapter drives a story whole");
  process.exit(failures ? 1 : 0);
})().catch((fault) => {
  console.error("FAILED:", fault && fault.stack ? fault.stack : fault);
  process.exit(1);
});
