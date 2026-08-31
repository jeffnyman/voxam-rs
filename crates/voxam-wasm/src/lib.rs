//! The interpreter as a browser module: stanzas in, stanzas out.
//!
//! This is the thinnest face Voxam has. It owns no DOM, draws
//! nothing, and knows no display library: it takes a story's bytes
//! and hands back the same GlkOte stanzas the CLI writes on stdout
//! and the shell sends down its pipe. A page wires those to
//! whatever display it likes.
//!
//! That thinness is the point. The seam between core and face has
//! always been the wire (PORT: the standing decision), and a
//! module that speaks the wire can be driven by the same typist
//! the certification sweeps drive -- so the browser build is
//! proven byte-identical to the stdio path rather than merely
//! looking right in a browser. A module that owned a display could
//! only ever be tested by eye.
//!
//! Stanzas cross as JSON *text*, not as JavaScript objects. The
//! wire's exact spelling -- insertion-ordered keys, compact
//! separators -- is what the sweeps diff, and handing objects
//! across would let the boundary respell them on the way out. The
//! host parses what it receives; a page that wants objects can
//! have them one `JSON.parse` later, and the bytes stay the bytes.
//!
//! ```js
//! const story = Story.open(bytes, { name: "Curses", seed: 1 });
//! story.onStanza(text => GlkOte.update(JSON.parse(text)));
//! story.send(JSON.stringify(initEvent));
//! story.free();
//! ```

use js_sys::{Function, Promise, Reflect};
use voxam_core::glkote::json::{Value, dumps, loads};
use voxam_core::session::{Opening, Sitting};
use voxam_core::zmachine::machine::Identity;
use wasm_bindgen::prelude::*;

/// One story, opened and standing.
#[wasm_bindgen]
pub struct Story {
    sitting: Sitting,
    listener: Option<Function>,
}

/// Read one optional field off an options object.
fn field(options: &JsValue, name: &str) -> Option<JsValue> {
    if options.is_undefined() || options.is_null() {
        return None;
    }

    Reflect::get(options, &JsValue::from_str(name))
        .ok()
        .filter(|held| !held.is_undefined() && !held.is_null())
}

#[wasm_bindgen]
impl Story {
    /// Open a story from its own bytes.
    ///
    /// The options, every one of them optional: `name`, the story's
    /// display name, which is only ever a label -- the bytes decide
    /// what the story actually is; `resources`, a second byte array
    /// for a Blorb that travels beside a bare story; `seed`, which
    /// makes a session reproducible to the last roll; and
    /// `interpreter` and `tandy`, the Z-Machine's §11.1.3-4 claim.
    ///
    /// A story that will not load throws here, and it is the only
    /// throw in the module: everything afterwards is answered in
    /// the protocol's own error stanza.
    pub fn open(bytes: Vec<u8>, options: &JsValue) -> Result<Story, JsError> {
        let name = field(options, "name")
            .and_then(|held| held.as_string())
            .unwrap_or_else(|| "story".to_string());
        let resources =
            field(options, "resources").map(|held| js_sys::Uint8Array::new(&held).to_vec());
        let seed = field(options, "seed")
            .and_then(|held| held.as_f64())
            .map(|held| held as u32);
        let identity = Identity {
            interpreter: field(options, "interpreter")
                .and_then(|held| held.as_f64())
                .map(|held| held as u8),
            tandy: field(options, "tandy").is_some_and(|held| held.is_truthy()),
        };

        let opening = Opening::of(&name, bytes, resources)
            .map_err(|error| JsError::new(&format!("voxam: {error}")))?;

        Ok(Self {
            sitting: opening.sitting(seed, identity),
            listener: None,
        })
    }

    /// Hear every stanza the story writes.
    #[wasm_bindgen(js_name = onStanza)]
    pub fn on_stanza(&mut self, listener: Function) {
        self.listener = Some(listener);
    }

    /// Hand the story one event, as JSON text. The init comes
    /// first, as it does on every other transport.
    ///
    /// The answer arrives at the listener rather than being
    /// returned, and it arrives in a microtask rather than inside
    /// this call. Both matter: GlkOte was designed against an
    /// answer that cannot arrive synchronously -- "the Game.accept
    /// call is a single HTTP request, and the data structure is a
    /// single HTTP response" -- and calling back into a display
    /// mid-dispatch would put it in a state no other transport
    /// ever does. Deferring also makes this module observably the
    /// same shape as the pipe and the socket, so a host's page
    /// code never forks on which transport it happens to have.
    pub fn send(&mut self, stanza: &str) {
        let answer = match loads(stanza) {
            Ok(Value::Object(held)) => self.sitting.answer(&held),
            Ok(_) => errored("voxam: a stanza is a JSON object"),
            Err(error) => errored(&format!("voxam: not JSON: {error}")),
        };

        self.deliver(&dumps(&Value::Object(answer)));
    }

    /// Hand one stanza to the listener, a microtask from now.
    ///
    /// The queueing is done by resolving a promise and letting the
    /// listener be its own continuation, which needs no Rust
    /// closure to be kept alive and so leaks nothing per turn.
    fn deliver(&self, text: &str) {
        let Some(listener) = &self.listener else {
            return;
        };

        let settled = Promise::resolve(&JsValue::from_str(text));

        // `settled.then(listener)`, reached by name so the
        // listener travels as itself rather than wrapped.
        if let Ok(then) = Reflect::get(&settled, &JsValue::from_str("then"))
            && let Ok(then) = then.dyn_into::<Function>()
        {
            let _ = then.call1(&settled, listener);
        }
    }
}

/// The protocol's own error stanza, for a stanza that never
/// reached the machine.
fn errored(message: &str) -> voxam_core::glkote::json::Object {
    let mut stanza = voxam_core::glkote::json::Object::new();

    stanza.set("type", "error");
    stanza.set("message", message);

    stanza
}
