//! What Glk needs from a display.
//!
//! The contract is deliberately narrow. Windows keep their own
//! contents, so a display renders the tree on flush and is asked
//! for input synchronously -- enough for a terminal, and the
//! arrangement cheapglk and glkterm use. A display that cannot
//! block -- one speaking the GlkOte protocol -- raises the
//! suspends flag instead, and glk_select stops asking: it records
//! what it waits for and the machine returns to its host, which
//! delivers the event through the library and steps on. The VM is
//! single-steppable for exactly that reason.
//!
//! Every capability defaults to "cannot": a display claims what it
//! can do by flipping a flag and overriding the methods behind it,
//! and the gestalt answers follow the flags, so a game never asks
//! for what the display never promised.
//!
//! Two reshapings from the reference, both for the borrow checker:
//! the tree-walking calls take the window map and an id rather
//! than a window object, and the reference's attach/post
//! back-reference -- a blocking display's only way to raise an
//! event mid-read -- becomes the `Asked::Instead` answer, which
//! carries the raised events back to the select loop directly.

use crate::glulx::glk::objects::{CHARACTER_CELL, Event, Metrics, SoundChannel, Window, WindowMap};
use crate::glulx::glk::resources::{ImageInfo, Resources};

/// A display's answer to an input ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked<T> {
    /// The input arrived.
    Answer(T),
    /// Nothing yet: these events happened instead -- a timer
    /// firing, most likely. The request stays pending and
    /// glk_select comes round again, which is what lets a timer
    /// event arrive without cancelling line input.
    Instead(Vec<Event>),
    /// No input can ever arrive: the session ends rather than
    /// hanging forever.
    End,
}

/// A display Glk can render into and read from.
pub trait Frontend {
    /// Whether timer events can fire here (Glk: Timer Events).
    fn timer_input(&self) -> bool {
        false
    }

    /// Whether the player can click in a grid or graphics window
    /// (Glk: Mouse Input Events).
    fn mouse_input(&self) -> bool {
        false
    }

    /// Whether the player can select a hyperlink (Glk: Accepting
    /// Hyperlink Events). Writing link values works everywhere
    /// regardless -- that is the separate Hyperlinks selector.
    fn hyperlink_input(&self) -> bool {
        false
    }

    /// Whether images and rectangles can be drawn (Glk: Graphics).
    fn graphics(&self) -> bool {
        false
    }

    /// Whether pictures can be set into a text buffer's flow (Glk:
    /// Graphics in Text Buffer Windows). True only where the
    /// display actually lays text around them.
    fn buffer_images(&self) -> bool {
        false
    }

    /// Whether sound resources can be played (Glk: Sound).
    fn sound(&self) -> bool {
        false
    }

    /// Whether input arrives from outside rather than from a read
    /// call. A blocking display is asked and answers on the spot;
    /// a suspending display is never asked -- glk_select records
    /// the wait, the machine returns to its host, and the host
    /// delivers the event through the library.
    fn suspends(&self) -> bool {
        false
    }

    /// Whether typed input is already visible without Glk
    /// reprinting it. A terminal echoes as the player types, so
    /// Glk echoing the line into the window as well would show it
    /// twice.
    fn echoes_input(&self) -> bool {
        false
    }

    /// The size of one character cell in the units size reports.
    fn metrics(&self) -> Metrics {
        CHARACTER_CELL
    }

    /// The metrics for one window; by default, the same for all.
    fn metrics_for(&self, window: &Window) -> Metrics {
        let _ = window;

        self.metrics()
    }

    /// The whole display as (width, height), in display units.
    fn size(&self) -> (i64, i64);

    /// Render the window tree. Called before every input ask.
    fn flush(&mut self, windows: &mut WindowMap, root: Option<u32>);

    /// Read a line for the window; Answer((text, terminator)). The
    /// terminator is 0 for an ordinary Return, or the keycode of
    /// whichever terminator key ended the line (Glk: Line Input
    /// Events).
    fn read_line(
        &mut self,
        windows: &mut WindowMap,
        window: u32,
        maxlen: u32,
    ) -> Asked<(String, u32)>;

    /// Read one keystroke, as a Glk character code.
    fn read_char(&mut self, windows: &mut WindowMap, window: u32) -> Asked<u32>;

    /// Ask for timer events every so often; zero stops them.
    fn set_timer(&mut self, millisecs: u32) {
        let _ = millisecs;
    }

    /// Whether two styles are visibly different in this window.
    fn style_distinguish(&self, window: &Window, first: u32, second: u32) -> bool {
        let _ = (window, first, second);

        false
    }

    /// Measure a style hint, or None if it cannot be measured.
    fn style_measure(&self, window: &Window, style: u32, hint: u32) -> Option<u32> {
        let _ = (window, style, hint);

        None
    }

    // The graphics contract is inert by default. A display that
    // sets the graphics flag overrides these; one that does not is
    // never asked, because the Graphics gestalt reports zero and
    // graphics windows refuse to open.

    /// Draw a picture; return whether it was drawn. The hyperlink
    /// is the window stream's current link value, so a picture
    /// drawn under a link stays clickable in a display that lays
    /// text around it (Glk: Graphics in Text Buffer Windows).
    #[allow(clippy::too_many_arguments)] // the drawing call's own shape
    fn draw_image(
        &mut self,
        windows: &mut WindowMap,
        window: u32,
        image: &ImageInfo,
        val1: i64,
        val2: i64,
        width: u32,
        height: u32,
        hyperlink: u32,
    ) -> bool {
        let _ = (windows, window, image, val1, val2, width, height, hyperlink);

        false
    }

    /// Erase a rectangle to the background (Glk: Graphics in
    /// Graphics Windows).
    fn erase_rect(
        &mut self,
        windows: &mut WindowMap,
        window: u32,
        left: i64,
        top: i64,
        width: u32,
        height: u32,
    ) {
        let _ = (windows, window, left, top, width, height);
    }

    /// Fill a rectangle with a color.
    #[allow(clippy::too_many_arguments)] // the fill call's own shape
    fn fill_rect(
        &mut self,
        windows: &mut WindowMap,
        window: u32,
        color: u32,
        left: i64,
        top: i64,
        width: u32,
        height: u32,
    ) {
        let _ = (windows, window, color, left, top, width, height);
    }

    /// Set the color future clears fill with.
    fn set_background_color(&mut self, windows: &mut WindowMap, window: u32, color: u32) {
        let _ = (windows, window, color);
    }

    /// Break text below margin images (Glk: Graphics in Text
    /// Buffer Windows).
    fn flow_break(&mut self, windows: &mut WindowMap, window: u32) {
        let _ = (windows, window);
    }

    // The sound contract, equally inert without the sound flag.
    // Each call names the channel by its arena key beside the
    // snapshot -- the reference passes the live object, whose
    // identity a snapshot cannot carry -- and a call that can
    // start a play takes the resources as an argument, the
    // state-view departure applied to sound.

    /// Begin playing; return whether it started.
    #[allow(clippy::too_many_arguments)] // the play call's own shape
    fn play_sound(
        &mut self,
        resources: &mut Resources,
        channel: u32,
        snapshot: &SoundChannel,
        sound: u32,
        repeats: u32,
        notify: u32,
    ) -> bool {
        let _ = (resources, channel, snapshot, sound, repeats, notify);

        false
    }

    /// Stop whatever the channel is playing.
    fn stop_sound(&mut self, channel: u32, snapshot: &SoundChannel) {
        let _ = (channel, snapshot);
    }

    /// Pause or resume the channel.
    fn pause_sound(
        &mut self,
        resources: &mut Resources,
        channel: u32,
        snapshot: &SoundChannel,
        paused: bool,
    ) {
        let _ = (resources, channel, snapshot, paused);
    }

    /// Change the channel's volume, over a duration if asked.
    fn set_volume(&mut self, channel: u32, snapshot: &SoundChannel, volume: u32, duration: u32) {
        let _ = (channel, snapshot, volume, duration);
    }

    /// Return a clicked position, or None if none can be.
    fn read_mouse(&mut self, window: u32) -> Option<(u32, u32)> {
        let _ = window;

        None
    }

    /// Return a selected link value, or None if none can be; zero
    /// means "not yet".
    fn read_hyperlink(&mut self, window: u32) -> Option<u32> {
        let _ = window;

        None
    }

    /// Ask the player for a filename; None cancels. Cancelling is
    /// always a legitimate answer (Glk: File References), so a
    /// display with no way to ask can simply inherit this. The
    /// window map rides along -- the tree-walk reshaping -- so a
    /// painted display can repaint the interrupted layout once
    /// the prompt is answered.
    fn prompt_file(&mut self, windows: &mut WindowMap, usage: u32, fmode: u32) -> Option<String> {
        let _ = windows;
        let _ = (usage, fmode);

        None
    }
}

/// A display that shows nothing and has no input.
///
/// Output is discarded; an input request ends the session, since a
/// game waiting for input that can never arrive would otherwise
/// hang forever.
#[derive(Debug, Default)]
pub struct NullFrontend;

impl Frontend for NullFrontend {
    /// A classic 80x24, so layout arithmetic stays sensible.
    fn size(&self) -> (i64, i64) {
        (80, 24)
    }

    fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

    fn read_line(
        &mut self,
        _windows: &mut WindowMap,
        _window: u32,
        _maxlen: u32,
    ) -> Asked<(String, u32)> {
        Asked::End
    }

    fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
        Asked::End
    }
}
