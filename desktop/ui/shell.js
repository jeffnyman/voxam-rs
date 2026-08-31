/* The desktop bridge: GlkOte in a Tauri webview, the machine on
   the far side of a pipe. The Rust core owns the child process;
   this file owns the ordering -- listeners before spawn, spawn
   before init -- and the page's own furniture (landing, ended
   bar, the story picker).

   Reload is restart: the chosen story lives in the Rust core and
   survives the page, exactly the web face's semantics. */

"use strict";

var invoke = window.__TAURI__.core.invoke;
var listen = window.__TAURI__.event.listen;
var dialog = window.__TAURI__.dialog;

/* Deliveries arriving before start_session resolves park here:
   the session id they must be filtered against is its return
   value, but a pre-wire refusal can outrun it. */
var sessionId = null;
var pending = [];
var faulted = false;
var opened = false;

/* The Display menu's words, spelled as CSS: the faces are stacks,
   the measures are column widths. */
var FACES = {
  serif: 'Palatino, Georgia, "Times New Roman", Times, serif',
  sans: '"Segoe UI", Helvetica, Arial, sans-serif',
  mono: 'Consolas, "Courier New", monospace'
};
var MEASURES = {
  narrow: "700px",
  standard: "900px",
  wide: "1200px",
  full: "100%"
};

/* The picker's story shapes; All files rides behind them so a
   renamed story is never unreachable. */
var FILTERS = [
  { name: "Stories", extensions: [
      "z1", "z2", "z3", "z4", "z5", "z6", "z7", "z8",
      "zblorb", "zlb", "ulx", "gblorb", "glb", "blorb", "blb"] },
  { name: "All files", extensions: ["*"] }
];

/* The file prompt's shapes, one per protocol filetype; All files
   rides behind each, since a player's old save may wear any name. */
var PROMPTED = {
  save: { name: "Saved games", extensions: ["glksave", "sav"] },
  transcript: { name: "Transcripts", extensions: ["txt", "log"] },
  command: { name: "Command records", extensions: ["txt", "rec"] },
  data: { name: "Data files", extensions: ["glkdata", "dat"] }
};

var Game = {
  /* Every GlkOte event becomes one line down the pipe. Rejections
     are swallowed: glkote.js hardcodes timer support, and a timer
     or arrange can still fire after the child has ended. Pictures
     arrive inline as data: urls in the updates themselves, so the
     shell owes no Blorb interface and no picture road. */
  accept: function(event) {
    invoke("send_stanza", { line: JSON.stringify(event) }).catch(function() {});
  },
  Dialog: {
    /* glkote.js asks whether the Dialog needs initing; a native
       picker has nothing to init. */
    inited: function() { return true; },

    /* The protocol's file prompt answered with a real picker over
       the very filesystem the interpreter writes: the chosen path
       travels back verbatim as the specialresponse value -- the
       machine side takes a plain string, and anything else reads
       as the cancel it is. This is the desktop's own power; the
       browser face cannot reach the player's disk. */
    open: function(tosave, usage, gameid, callback) {
      var shaped = PROMPTED[usage];
      var filters = shaped
        ? [shaped, { name: "All files", extensions: ["*"] }]
        : [{ name: "All files", extensions: ["*"] }];
      var asked = tosave
        ? dialog.save({ filters: filters })
        : dialog.open({ filters: filters });

      asked.then(function(path) {
        callback(path || null);
      }).catch(function() {
        callback(null);
      });
    }
  }
};
window.Game = Game;

/* A finished play reports back down the same pipe every other
   event takes. */
VoxamAudio.start(function(event) { Game.accept(event); });

/* The picker starts at the stories' own home -- a pinned folder,
   or the last story's -- so a save to some other corner of the
   disk never drags the story picker after it. */
function chooseStory() {
  invoke("story_home").then(function(home) {
    var asked = { filters: FILTERS };

    if (home) asked.defaultPath = home;

    return dialog.open(asked);
  }).then(function(path) {
    if (!path) return;

    invoke("set_story", { path: path }).then(function() {
      location.reload();
    });
  });
}

/* Dress the page in the settings' word, then poke a resize so
   GlkOte re-measures its metrics -- the machines take the new
   arrangement live, no restart owed. Pre-init the poke is a
   harmless no-op; the dress still paints the landing. */
function dressed(display) {
  var root = document.documentElement.style;

  root.setProperty("--story-face", FACES[display.face] || FACES.serif);
  root.setProperty("--story-size", display.size + "px");
  root.setProperty("--grid-size", (display.size - 1) + "px");
  root.setProperty("--measure", MEASURES[display.measure] || MEASURES.standard);
  document.body.className = "theme-" + display.theme;

  window.dispatchEvent(new Event("resize"));
}

/* Open or close the side panes, then poke a resize so GlkOte
   re-measures the story's column in the space that is left. The
   map itself is drawn from whatever the Rust side last sent. */
function paned(panes) {
  document.body.classList.toggle("pane-map", !!panes.map);
  document.body.classList.toggle("pane-notes", !!panes.notes);
  document.body.classList.toggle("paned", !!(panes.map || panes.notes));

  window.dispatchEvent(new Event("resize"));

  if (panes.map) drawMap();
}

/* Notes are saved a breath after typing stops rather than on
   every keystroke: the file is the player's, and rewriting it
   thirty times a sentence earns nothing. */
var NOTES_REST = 600;
var notesTimer = null;

function noteChanged() {
  if (notesTimer) clearTimeout(notesTimer);

  notesTimer = setTimeout(function() {
    notesTimer = null;
    invoke("set_notes", { text: document.getElementById("notes").value })
      .catch(function() {});
  }, NOTES_REST);
}

/* Whatever is pending, written now: a shell that is closing has
   no breath left to wait for. */
function noteSettled() {
  if (!notesTimer) return;

  clearTimeout(notesTimer);
  notesTimer = null;
  invoke("set_notes", { text: document.getElementById("notes").value })
    .catch(function() {});
}

/* Stamp the room the player stands in at the cursor -- the note a
   player is usually about to write anyway. */
function stampHere() {
  invoke("bearings").then(function(bearings) {
    var where = bearings && bearings.location ? bearings.location.name : null;

    if (!where) return;

    var box = document.getElementById("notes");
    var at = box.selectionStart;
    var before = box.value.slice(0, at);
    var after = box.value.slice(box.selectionEnd);
    var written = (before && !before.endsWith("\n") ? "\n" : "") + where + ": ";

    box.value = before + written + after;
    box.focus();
    box.selectionStart = box.selectionEnd = before.length + written.length;

    noteChanged();
  });
}

/* The last map the shell sent, kept so opening the pane draws
   immediately rather than waiting for the next room. */
var mapHeld = null;

function drawMap() {
  if (!mapHeld || !document.body.classList.contains("pane-map")) return;

  VoxamMap.draw(mapHeld, document.getElementById("mapdraw"));
}

function stranded(message) {
  document.getElementById("loadingpane").style.display = "none";
  document.getElementById("note").textContent = message;
  document.getElementById("landing").style.display = "flex";
}

function deliver(kind, payload) {
  if (sessionId === null) {
    pending.push([kind, payload]);
    return;
  }

  /* A dead session's last words: the reload that replaced it
     attached fresh listeners before its pumps wound down. */
  if (payload.id !== sessionId) return;

  if (kind === "stanza") {
    /* Sounds ride the update in VOXAM's own dialect; the audio
       module reads them before GlkOte draws the rest. */
    VoxamAudio.update(payload.stanza);
    GlkOte.update(payload.stanza);
  } else if (kind === "fault") {
    /* Refusals and crashes both land in GlkOte's error pane,
       which is plain DOM and safe before init has ever run. */
    faulted = true;
    document.getElementById("loadingpane").style.display = "none";
    GlkOte.error(payload.text);
  } else if (kind === "ended" && !faulted) {
    /* The vendored glkote.js ignores the update's exit flag, so
       this bar is the only end-of-story signal the player gets. */
    document.getElementById("endedbar").style.display = "block";
  }
}

window.addEventListener("DOMContentLoaded", function() {
  document.getElementById("open").addEventListener("click", chooseStory);
  document.getElementById("reopen").addEventListener("click", chooseStory);
  document.getElementById("notes").addEventListener("input", noteChanged);
  document.getElementById("notes").addEventListener("blur", noteSettled);
  document.getElementById("stamp").addEventListener("click", stampHere);

  /* A window closing, or a page reloading into a new story, must
     not take the last sentence with it. */
  window.addEventListener("beforeunload", noteSettled);

  listen("menu-open", chooseStory);
  listen("menu-restart", function() {
    if (opened) location.reload();
  });
  listen("menu-home", function() {
    dialog.open({ directory: true }).then(function(path) {
      if (path) invoke("set_home", { path: path });
    });
  });
  listen("menu-follow", function() {
    invoke("set_home", { path: null });
  });
  listen("display", function(event) {
    dressed(event.payload);
  });
  listen("panes", function(event) {
    paned(event.payload);
  });

  /* Forgetting a map is an hour's walking undone and nothing
     brings it back, so it is asked before it is done. */
  listen("menu-forget-map", function() {
    dialog.ask(
      "The rooms and passages walked in this story will be forgotten. " +
        "Notes are kept.",
      { title: "Forget this map?", kind: "warning", okLabel: "Forget" }
    ).then(function(willing) {
      if (willing) invoke("forget_map");
    }).catch(function() {});
  });

  /* The map arrives only when it has grown: the shell reads the
     sidecar off each update and sends the map on when a room or
     a passage is new. */
  listen("map", function(event) {
    mapHeld = event.payload;
    drawMap();
  });

  /* The dress arrives before the story: GlkOte's init measures
     the page, so the page must already wear its type and ink. */
  invoke("display_settings").then(function(display) {
    dressed(display);

    /* The panes are dressed before the story too: GlkOte's init
       measures the page, so the column must already stand in the
       width it is going to keep. */
    return invoke("open_panes");
  }).then(function(panes) {
    paned(panes);

    return invoke("current_story");
  }).then(function(story) {
    if (!story) return;

    opened = true;
    document.getElementById("landing").style.display = "none";

    /* listen() registers over IPC and resolves later; awaiting all
       three before the spawn is what keeps an instant refusal line
       from being emitted into the void. */
    Promise.all([
      listen("stanza", function(event) { deliver("stanza", event.payload); }),
      listen("fault", function(event) { deliver("fault", event.payload); }),
      listen("ended", function(event) { deliver("ended", event.payload); })
    ]).then(function() {
      return invoke("start_session");
    }).then(function(id) {
      sessionId = id;

      var parked = pending;
      pending = [];
      parked.forEach(function(entry) { deliver(entry[0], entry[1]); });

      /* A fault that beat the id here means the story never stood
         up; init would only send its stanza into a dead pipe. */
      if (!faulted) GlkOte.init();

      /* The map and notes this story was left with, from the last
         time it was played: both panes show them before the first
         step is taken. */
      invoke("walked_map").then(function(map) {
        mapHeld = map;
        drawMap();
      });

      invoke("story_notes").then(function(text) {
        document.getElementById("notes").value = text || "";
      });
    }).catch(function(message) {
      stranded(String(message));
    });
  });
});
