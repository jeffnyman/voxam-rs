/* Wire a Voxam story to GlkOte. No wasm in here.

   GlkOte drives a `Game` object: it hands every event to
   `Game.accept`, and expects `GlkOte.update` to be called with
   the answer whenever the answer arrives. That contract is the
   same whatever carries the stanzas -- an HTTP POST for the
   browser face, a pipe for the desktop shell, a postMessage for
   an editor's webview -- so this file is the thirty lines that
   would otherwise be written once per host, and it is deliberately
   ignorant of which host it is in.

   Pictures and sounds ride inside the updates as data: urls, so
   no Blorb interface is owed here and no resource road is needed.

   The file prompt is the one thing a host must answer for itself:
   a page with nowhere to save should cancel, and a host with
   somewhere (an editor's workspace, a shell's disk) should hand
   back a name. Cancelling is honest -- the story prints its own
   "Failed." and plays on. */

"use strict";

function voxamGame(story, hooks) {
  hooks = hooks || {};

  var Game = {
    /* Every GlkOte event becomes one stanza handed to the story.
       Nothing is returned: the answer arrives at onStanza below,
       a microtask later, which is the same shape every other
       transport has. */
    accept: function (event) {
      story.send(JSON.stringify(event));
    },
  };

  /* The protocol's file prompt. Without a Dialog, GlkOte has no
     way to answer one at all; with this one, a host that offers
     no `prompt` hook cancels every ask, and the story says so in
     its own words. */
  Game.Dialog = {
    inited: function () {
      return true;
    },
    open: function (tosave, usage, gameid, callback) {
      if (!hooks.prompt) {
        callback(null);
        return;
      }

      hooks.prompt(tosave, usage, callback);
    },
  };

  story.onStanza(function (text) {
    var update = JSON.parse(text);

    /* Sounds ride the update in Voxam's own dialect, and the
       audio module reads them before GlkOte draws the rest. */
    if (typeof VoxamAudio !== "undefined") VoxamAudio.update(update);

    GlkOte.update(update);
  });

  if (typeof VoxamAudio !== "undefined") {
    VoxamAudio.start(function (event) {
      Game.accept(event);
    });
  }

  return Game;
}

if (typeof window !== "undefined") window.voxamGame = voxamGame;
if (typeof module !== "undefined" && module.exports) module.exports = { voxamGame };
