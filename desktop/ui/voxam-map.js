/* The map pane: the walked map, drawn as SVG.

   All the thinking happened in the shell's Rust -- which rooms
   exist, where they sit, which passages are real -- so this file
   only draws. It is handed a map (rooms with cells, directed
   passages, the room the player stands in) and turns it into
   shapes, fitting the view to whatever has been walked so far.

   The drawing conventions, each answering something the model
   decided:

   - A compass passage is a plain line between two cells, because
     the model already placed those cells by that direction.
   - Up, down, in and out have no direction on the plane, so they
     are drawn dashed and lettered (U, D, I, O) rather than
     pretending to a bearing they do not have.
   - A passage the model could not name is dotted: the player
     travelled it, and the map says so without claiming to know
     how.
   - One-way passages carry an arrowhead. Two rooms joined both
     ways are the ordinary case and are drawn plain, so the
     arrowheads that remain are worth noticing. */

"use strict";

var VoxamMap = (function() {
  /* The cell grid, in pixels. Rooms are drawn smaller than their
     cell so passages have room to show between them. */
  var CELL = 74;
  var ROOM_W = 54;
  var ROOM_H = 30;
  var MARGIN = 26;

  var LETTERED = { up: "U", down: "D", in: "I", out: "O" };

  function svgNode(name, attrs) {
    var node = document.createElementNS("http://www.w3.org/2000/svg", name);

    for (var key in attrs) {
      if (Object.prototype.hasOwnProperty.call(attrs, key)) {
        node.setAttribute(key, attrs[key]);
      }
    }

    return node;
  }

  /* A cell's centre in drawing coordinates. */
  function centre(room, bounds) {
    return {
      x: (room.x - bounds.left) * CELL + CELL / 2 + MARGIN,
      y: (room.y - bounds.top) * CELL + CELL / 2 + MARGIN
    };
  }

  /* Where a line from one room to another should stop: at the
     edge of the destination's box, not its centre, so an
     arrowhead lands where the room begins. */
  function meeting(from, to) {
    var dx = to.x - from.x;
    var dy = to.y - from.y;
    var span = Math.sqrt(dx * dx + dy * dy);

    if (span === 0) return to;

    var stepX = dx / span;
    var stepY = dy / span;
    /* The box's own half-diagonal in this direction, which for a
       rectangle is whichever edge the ray leaves through. */
    var reach = Math.min(
      Math.abs(stepX) > 0.001 ? (ROOM_W / 2) / Math.abs(stepX) : Infinity,
      Math.abs(stepY) > 0.001 ? (ROOM_H / 2) / Math.abs(stepY) : Infinity
    );

    return { x: to.x - stepX * reach, y: to.y - stepY * reach };
  }

  /* A room's name, trimmed to what fits the box. */
  function fitted(name) {
    if (name.length <= 11) return name;

    return name.slice(0, 10) + "…";
  }

  function draw(map, into) {
    into.textContent = "";

    if (map.unreliable) {
      var told = document.createElement("p");

      told.className = "mapnote";
      told.textContent =
        "This story does not report where the player is, so no map " +
        "can be drawn for it.";
      into.appendChild(told);

      return;
    }

    var rooms = Object.keys(map.rooms).map(function(key) {
      return map.rooms[key];
    });

    if (!rooms.length) {
      var empty = document.createElement("p");

      empty.className = "mapnote";
      empty.textContent = "The map fills in as you walk.";
      into.appendChild(empty);

      return;
    }

    var bounds = {
      left: Math.min.apply(null, rooms.map(function(r) { return r.x; })),
      right: Math.max.apply(null, rooms.map(function(r) { return r.x; })),
      top: Math.min.apply(null, rooms.map(function(r) { return r.y; })),
      bottom: Math.max.apply(null, rooms.map(function(r) { return r.y; }))
    };
    var width = (bounds.right - bounds.left + 1) * CELL + MARGIN * 2;
    var height = (bounds.bottom - bounds.top + 1) * CELL + MARGIN * 2;

    var svg = svgNode("svg", {
      viewBox: "0 0 " + width + " " + height,
      width: width,
      height: height,
      class: "mapdrawing"
    });

    /* One arrowhead, shared by every one-way passage. */
    var defs = svgNode("defs", {});
    var marker = svgNode("marker", {
      id: "voxam-arrow",
      viewBox: "0 0 8 8",
      refX: "7",
      refY: "4",
      markerWidth: "5",
      markerHeight: "5",
      orient: "auto-start-reverse"
    });

    marker.appendChild(svgNode("path", { d: "M 0 1 L 7 4 L 0 7 z", class: "maparrow" }));
    defs.appendChild(marker);
    svg.appendChild(defs);

    /* Which pairs are joined both ways: those need no arrowhead,
       which leaves the arrows that remain worth noticing. */
    var both = {};

    map.edges.forEach(function(edge) {
      both[edge.from + ">" + edge.to] = true;
    });

    var passages = svgNode("g", { class: "mappassages" });

    map.edges.forEach(function(edge) {
      var from = map.rooms[edge.from];
      var to = map.rooms[edge.to];

      if (!from || !to) return;

      var a = centre(from, bounds);
      var b = meeting(a, centre(to, bounds));
      var kind = edge.step.kind;
      /* A passage between neighbouring cells is a straight line
         and means what it looks like. A longer one exists because
         its far room was already placed elsewhere -- rooms never
         move once drawn -- so it is bowed and faded rather than
         ruled straight through every room in between, which would
         read as passing through them. */
      var reach = Math.max(
        Math.abs(to.x - from.x),
        Math.abs(to.y - from.y)
      );
      var line;

      if (reach > 1) {
        var midX = (a.x + b.x) / 2;
        var midY = (a.y + b.y) / 2;
        var awayX = -(b.y - a.y);
        var awayY = b.x - a.x;
        var span = Math.sqrt(awayX * awayX + awayY * awayY) || 1;
        var bow = Math.min(18 + reach * 3, 60);

        line = svgNode("path", {
          d:
            "M " + a.x + " " + a.y +
            " Q " + (midX + (awayX / span) * bow) + " " +
            (midY + (awayY / span) * bow) + " " + b.x + " " + b.y,
          class: "mappassage stretched kind-" + kind
        });
      } else {
        line = svgNode("line", {
          x1: a.x,
          y1: a.y,
          x2: b.x,
          y2: b.y,
          class: "mappassage kind-" + kind
        });
      }

      if (!both[edge.to + ">" + edge.from]) {
        line.setAttribute("marker-end", "url(#voxam-arrow)");
      }

      passages.appendChild(line);

      /* A passage with no bearing says which it was in letters. */
      if (LETTERED[kind]) {
        var mid = svgNode("text", {
          x: (a.x + b.x) / 2,
          y: (a.y + b.y) / 2 - 3,
          class: "maplabel"
        });

        mid.textContent = LETTERED[kind];
        passages.appendChild(mid);
      }
    });

    svg.appendChild(passages);

    var boxes = svgNode("g", { class: "maprooms" });

    rooms.forEach(function(room) {
      var at = centre(room, bounds);
      var here = map.here === room.object;
      var box = svgNode("rect", {
        x: at.x - ROOM_W / 2,
        y: at.y - ROOM_H / 2,
        width: ROOM_W,
        height: ROOM_H,
        rx: 3,
        class: "maproom" + (here ? " here" : "")
      });
      var name = svgNode("text", {
        x: at.x,
        y: at.y + 4,
        class: "mapname" + (here ? " here" : "")
      });

      name.textContent = fitted(room.name);
      /* The whole name, for a pointer that lingers. */
      box.appendChild(svgNode("title", {})).textContent = room.name;

      boxes.appendChild(box);
      boxes.appendChild(name);
    });

    svg.appendChild(boxes);
    into.appendChild(svg);

    /* Follow the player: the room they stand in is what the pane
       should be looking at after a walk. */
    var standing = map.rooms[map.here];

    if (standing) {
      var at = centre(standing, bounds);

      into.scrollLeft = at.x - into.clientWidth / 2;
      into.scrollTop = at.y - into.clientHeight / 2;
    }
  }

  return { draw: draw };
})();

window.VoxamMap = VoxamMap;
