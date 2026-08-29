//! The Version 6 window ledger: §8.8's eight windows as pure
//! state.
//!
//! Version 6 keeps eight windows, each with a position and size in
//! units, a cursor, four attribute flags, and eighteen numbered
//! properties (§8.8.3). The properties are machine state before
//! they are anything visual -- get_wind_prop reads them and
//! ZIPTEST's window tests do little else -- so this ledger keeps
//! them faithfully with no glass in sight. The character frontends
//! keep rendering windows 0 and 1 as they always have.

use crate::errors::VoxamError;

/// Eight windows, numbered 0 to 7; the code -3 -- the unsigned
/// word 65533, as an operand carries it -- means "the currently
/// selected window" (§8.8.3).
const WINDOW_COUNT: usize = 8;
pub const CURRENT_WINDOW: i32 = -3;
const UNSIGNED_CURRENT: i32 = 0xFFFD;

/// §8.8.3.2's eighteen properties, by number. Properties 0 to 15
/// are writeable with put_wind_prop; the true colours must not be
/// written.
pub const Y_COORDINATE: u16 = 0;
pub const X_COORDINATE: u16 = 1;
pub const Y_SIZE: u16 = 2;
pub const X_SIZE: u16 = 3;
pub const Y_CURSOR: u16 = 4;
pub const X_CURSOR: u16 = 5;
pub const FONT_NUMBER: u16 = 12;
pub const FONT_SIZE: u16 = 13;
pub const ATTRIBUTES: u16 = 14;
pub const COLOUR_DATA: u16 = 11;
pub const LEFT_MARGIN: u16 = 6;
pub const RIGHT_MARGIN: u16 = 7;
const PROPERTY_COUNT: u16 = 18;
const LAST_WRITABLE: u16 = 15;

/// §8.8.3.1's four attributes, as the flag bits window_style
/// speaks. Window 0 begins with all four -- running text wraps,
/// scrolls, and echoes -- while the others begin buffered only,
/// exactly the defaults §8.8.3.1.2's example describes.
const WRAPPING: u16 = 1;
const SCROLLING: u16 = 2;
const TRANSCRIPTING: u16 = 4;
const BUFFERING: u16 = 8;

/// window_style's operations on those flags (§15 window_style).
const STYLE_SET: u16 = 0;
const STYLE_ON: u16 = 1;
const STYLE_OFF: u16 = 2;
const STYLE_REVERSE: u16 = 3;

/// The font size property carries height in its upper byte and
/// width in its lower (§8.8.3.2.5); colour data likewise carries
/// background high and foreground low (§8.8.3.2.4).
const BYTE_SHIFT: u16 = 8;

fn screen_error(message: String) -> VoxamError {
    VoxamError::ZMachineScreen(message)
}

/// Eight windows' worth of §8.8 bookkeeping, and the selection.
pub struct WindowLedger {
    /// The currently selected window's number, which the code -3
    /// resolves to (§8.8.3).
    pub selected: u16,
    windows: [[u16; PROPERTY_COUNT as usize]; WINDOW_COUNT],
}

impl WindowLedger {
    /// Set every window to its §8.8 boot state, in units.
    pub fn new(
        height: u16,
        width: u16,
        foreground: u16,
        background: u16,
        font_width: u16,
        font_height: u16,
    ) -> Self {
        let mut windows = [[0u16; PROPERTY_COUNT as usize]; WINDOW_COUNT];

        for (number, window) in windows.iter_mut().enumerate() {
            window[Y_COORDINATE as usize] = 1;
            window[X_COORDINATE as usize] = 1;
            window[Y_CURSOR as usize] = 1;
            window[X_CURSOR as usize] = 1;
            window[FONT_NUMBER as usize] = 1;
            window[FONT_SIZE as usize] = (font_height << BYTE_SHIFT) | font_width;
            window[COLOUR_DATA as usize] = (background << BYTE_SHIFT) | foreground;
            window[ATTRIBUTES as usize] = BUFFERING;

            if number == 0 {
                window[Y_SIZE as usize] = height;
                window[X_SIZE as usize] = width;
                window[ATTRIBUTES as usize] = WRAPPING | SCROLLING | TRANSCRIPTING | BUFFERING;
            } else if number == 1 {
                // Screen-wide and flat: §8.8.4.1's split tiles this
                // window against 0 without touching widths.
                window[X_SIZE as usize] = width;
            }
        }

        Self {
            selected: 0,
            windows,
        }
    }

    /// The window a number names, -3 meaning the selected one.
    pub fn resolve(&self, window: i32) -> Result<u16, VoxamError> {
        if window == CURRENT_WINDOW || window == UNSIGNED_CURRENT {
            return Ok(self.selected);
        }

        if (0..WINDOW_COUNT as i32).contains(&window) {
            return Ok(window as u16);
        }

        Err(screen_error(format!(
            "window {window} is not one of the eight (§8.8.3)"
        )))
    }

    /// Read one §8.8.3.2 property, as get_wind_prop does.
    pub fn property(&self, window: i32, number: u16) -> Result<u16, VoxamError> {
        let target = self.resolve(window)?;

        Ok(self.windows[usize::from(target)][usize::from(known(number)?)])
    }

    /// Write one property, as put_wind_prop does (§8.8.3.2); the
    /// true colour properties "must not be written".
    pub fn write_property(
        &mut self,
        window: i32,
        number: u16,
        value: u16,
    ) -> Result<(), VoxamError> {
        if known(number)? > LAST_WRITABLE {
            return Err(screen_error(format!(
                "window property {number} is a true colour, which must not be \
                 written (§8.8.3.2)"
            )));
        }

        let target = self.resolve(window)?;
        self.windows[usize::from(target)][usize::from(number)] = value;

        Ok(())
    }

    /// Place a window's top left at (y, x) in units (§15).
    pub fn r#move(&mut self, window: i32, y: u16, x: u16) -> Result<(), VoxamError> {
        let target = usize::from(self.resolve(window)?);
        self.windows[target][Y_COORDINATE as usize] = y;
        self.windows[target][X_COORDINATE as usize] = x;

        Ok(())
    }

    /// Set a window's size in units, as window_size does (§15).
    pub fn resize(&mut self, window: i32, height: u16, width: u16) -> Result<(), VoxamError> {
        let target = usize::from(self.resolve(window)?);
        self.windows[target][Y_SIZE as usize] = height;
        self.windows[target][X_SIZE as usize] = width;

        Ok(())
    }

    /// Change a window's attribute flags (§15 window_style):
    /// operation 0 sets the flags outright, 1 turns the given bits
    /// on, 2 turns them off, and 3 reverses them.
    pub fn restyle(&mut self, window: i32, flags: u16, operation: u16) -> Result<(), VoxamError> {
        let target = usize::from(self.resolve(window)?);
        let attributes = &mut self.windows[target][ATTRIBUTES as usize];

        match operation {
            STYLE_SET => *attributes = flags,
            STYLE_ON => *attributes |= flags,
            STYLE_OFF => *attributes &= !flags,
            STYLE_REVERSE => *attributes ^= flags,
            _ => {
                return Err(screen_error(format!(
                    "window_style operation {operation} is not one of §15's four \
                     (set, on, off, reverse)"
                )));
            }
        }

        Ok(())
    }

    /// Set a window's margin sizes, in units (§8.8.3.2.1).
    pub fn set_margins(&mut self, window: i32, left: u16, right: u16) -> Result<(), VoxamError> {
        let target = usize::from(self.resolve(window)?);
        self.windows[target][LEFT_MARGIN as usize] = left;
        self.windows[target][RIGHT_MARGIN as usize] = right;

        Ok(())
    }
}

/// Police a property number against §8.8.3.2's eighteen.
fn known(number: u16) -> Result<u16, VoxamError> {
    if number >= PROPERTY_COUNT {
        return Err(screen_error(format!(
            "window property {number} is not one of §8.8.3.2's eighteen"
        )));
    }

    Ok(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> WindowLedger {
        WindowLedger::new(255, 80, 9, 2, 1, 1)
    }

    #[test]
    fn boots_to_the_section_8_8_state() {
        let ledger = ledger();

        assert_eq!(ledger.property(0, Y_SIZE).unwrap(), 255);
        assert_eq!(ledger.property(0, X_SIZE).unwrap(), 80);
        assert_eq!(ledger.property(0, ATTRIBUTES).unwrap(), 15);
        assert_eq!(ledger.property(1, X_SIZE).unwrap(), 80);
        assert_eq!(ledger.property(1, Y_SIZE).unwrap(), 0);
        assert_eq!(ledger.property(1, ATTRIBUTES).unwrap(), 8);
        assert_eq!(ledger.property(2, FONT_SIZE).unwrap(), 0x0101);
        assert_eq!(ledger.property(3, COLOUR_DATA).unwrap(), 0x0209);
    }

    #[test]
    fn minus_three_names_the_selected_window() {
        let mut ledger = ledger();
        ledger.selected = 5;

        assert_eq!(ledger.resolve(-3).unwrap(), 5);
        assert_eq!(ledger.resolve(0xFFFD).unwrap(), 5);
        assert_eq!(ledger.resolve(2).unwrap(), 2);
        assert!(ledger.resolve(8).is_err());
    }

    #[test]
    fn styles_apply_their_four_operations() {
        let mut ledger = ledger();

        ledger.restyle(2, 0b0101, STYLE_SET).unwrap();
        assert_eq!(ledger.property(2, ATTRIBUTES).unwrap(), 0b0101);

        ledger.restyle(2, 0b0010, STYLE_ON).unwrap();
        assert_eq!(ledger.property(2, ATTRIBUTES).unwrap(), 0b0111);

        ledger.restyle(2, 0b0001, STYLE_OFF).unwrap();
        assert_eq!(ledger.property(2, ATTRIBUTES).unwrap(), 0b0110);

        ledger.restyle(2, 0b1111, STYLE_REVERSE).unwrap();
        assert_eq!(ledger.property(2, ATTRIBUTES).unwrap(), 0b1001);

        assert!(ledger.restyle(2, 0, 4).is_err());
    }

    #[test]
    fn true_colours_refuse_writes() {
        let mut ledger = ledger();

        assert!(ledger.write_property(0, 16, 1).is_err());
        assert!(ledger.write_property(0, 18, 1).is_err());
        assert!(ledger.write_property(0, 15, 1).is_ok());
    }
}
