//! The Å-machine at the terminal: the plain voice, spoken live.
//!
//! The drill is the reference Node frontend's own, certified
//! against its transcripts: a line wait takes the typed line
//! whole, a key wait takes it a keypress at a time with a return
//! to finish, and the terminal's own echo stands in for the
//! readline echo the transcripts carry. The voice here keeps
//! files too: a save or restore asks for its filename on the
//! spot, the blocking face's privilege (Aa-machine: Savefile).
//!
//! At a real terminal the voice also dresses the text: the LOOK
//! chunk's classes ride every span, div, and body call, and the
//! ones a terminal can honor -- bold, italic, and color -- land
//! as the terminal's own attributes, with italics worn as
//! underlines, the Dialog debugger's own precedent. The escapes
//! are injected past the word-wrapper, which counts columns and
//! must never see a zero-width code. A piped session stays plain,
//! so every certified transcript still matches byte for byte, and
//! VM_INFO answers the styling and color questions truthfully per
//! stream (Aa-machine: VM_INFO).
//!
//! Two reshapings from the reference, in the stdio display's
//! manner: the StyledVoice layer folds in here, its only wearer,
//! and played()'s live-stream defaults move to the caller -- the
//! input source, writer, and filename prompt arrive as explicit
//! seams (the CLI wires the real streams in; a test wires
//! whatever it likes).

use std::io::Write;

use crate::aamachine::machine::{Machine, Wait};
use crate::aamachine::output::{PlainVoice, Voice, Wardrobe};
use crate::aamachine::story::Story;
use crate::errors::VoxamError;

/// The suffix a bare savefile name gains, the house courtesy.
pub const SUFFIX: &str = ".aasave";

/// The filename prompt: asked with the prompt text, it answers
/// the player's name for the file.
pub type Asked = Box<dyn FnMut(&str) -> String>;

/// The plain voice with a terminal's file-keeping manners.
///
/// Dressed, it also wears the LOOK chunk's styles as terminal
/// attributes and answers VM_INFO's styling and color questions
/// with yes -- the honesty gate being that only a real terminal
/// is ever dressed.
pub struct TerminalVoice {
    plain: PlainVoice,
    wardrobe: Wardrobe,
    writer: Box<dyn Write>,
    asked: Asked,
    mark: usize,
    dressed: bool,
}

impl TerminalVoice {
    /// Speak at a width, through a writer, asking by a prompt.
    pub fn new(
        story: &Story,
        width: i64,
        writer: Box<dyn Write>,
        asked: Asked,
        dressed: bool,
    ) -> Result<Self, VoxamError> {
        let plain = PlainVoice::new(story)?.sized(width);
        let wardrobe = Wardrobe::new(&plain.styles);

        Ok(Self {
            plain,
            wardrobe,
            writer,
            asked,
            mark: 0,
            dressed,
        })
    }

    /// Everything said so far, the pending word flushed out.
    pub fn told(&mut self) -> &str {
        self.plain.told()
    }

    /// Note that a sent line's echo reset the cursor.
    pub fn prompted(&mut self) {
        self.plain.prompted();
    }

    /// Take every attribute off, leaving the terminal clean.
    pub fn undressed(&mut self) {
        if self.dressed {
            self.plain.flushed();
            self.plain.told.push_str("\x1b[0m");
        }
    }

    /// Land the current dress on the terminal, if one may land.
    ///
    /// The escape is injected past the word-wrapper the way an
    /// echo is: the wrapper counts columns, and a dress is
    /// zero-width.
    fn fitted(&mut self) {
        if !self.dressed || self.plain.hidden {
            return;
        }

        let (bold, italic, reverse, ink, paper) = self.wardrobe.folded();
        let mut pieces = vec!["0".to_string()];

        if bold {
            pieces.push("1".to_string());
        }

        // Italics wear underlines, the Dialog debugger's own
        // rendering -- every terminal draws an underline, which is
        // what keeps the spec's distinguishability bar cleared
        // everywhere (Aa-machine: VM_INFO).
        if italic {
            pieces.push("4".to_string());
        }

        if reverse {
            pieces.push("7".to_string());
        }

        if let Some((r, g, b)) = ink {
            pieces.push(format!("38;2;{r};{g};{b}"));
        }

        if let Some((r, g, b)) = paper {
            pieces.push(format!("48;2;{r};{g};{b}"));
        }

        self.plain.flushed();
        self.plain
            .told
            .push_str(&format!("\x1b[{}m", pieces.join(";")));
    }

    /// Write everything told since the last pour.
    pub fn poured(&mut self) {
        self.plain.flushed();

        let _ = self
            .writer
            .write_all(&self.plain.told.as_bytes()[self.mark..]);
        let _ = self.writer.flush();

        self.mark = self.plain.told.len();
    }

    /// One filename from the player, the pending story poured
    /// first.
    ///
    /// A bare name gains the .aasave suffix; the player's own
    /// dotted path is honored whole.
    fn named(&mut self, prompt: &str) -> String {
        self.line();
        self.poured();

        let name = (self.asked)(prompt).trim().to_string();

        self.prompted();

        if !name.is_empty() && !name.contains('.') {
            return format!("{name}{SUFFIX}");
        }

        name
    }
}

impl Voice for TerminalVoice {
    fn has_saves(&self) -> bool {
        true
    }

    fn has_styles(&self) -> bool {
        self.dressed
    }

    fn has_color(&self) -> bool {
        self.dressed
    }

    fn say(&mut self, text: &str) {
        self.plain.say(text);
    }

    fn nbsp(&mut self) {
        self.plain.nbsp();
    }

    fn space(&mut self) {
        self.plain.space();
    }

    fn spaces(&mut self, count: i64) {
        self.plain.spaces(count);
    }

    fn line(&mut self) {
        self.plain.line();
    }

    fn par(&mut self) {
        self.plain.par();
    }

    /// Open a div: the break as ever, then its class's dress.
    fn enter_div(&mut self, style: i64) {
        self.plain.enter_div(style);
        self.wardrobe.entered(style);
        self.fitted();
    }

    /// Close a div: the dress beneath first, then the break.
    fn leave_div(&mut self, style: i64) {
        self.wardrobe.left();
        self.fitted();
        self.plain.leave_div(style);
    }

    /// Open a span, wearing its class's dress.
    fn enter_span(&mut self, style: i64) {
        self.wardrobe.entered(style);
        self.fitted();
    }

    /// Close the span, the dress beneath restored.
    fn leave_span(&mut self) {
        self.wardrobe.left();
        self.fitted();
    }

    /// Dress the document body; every later dress layers on it.
    fn set_body(&mut self, style: i64) {
        self.wardrobe.bodied(style);
        self.fitted();
    }

    fn enter_status(&mut self, area: i64, style: i64) {
        self.plain.enter_status(area, style);
    }

    fn leave_status(&mut self) {
        self.plain.leave_status();
    }

    fn enter_link(&mut self, words: &str) {
        self.plain.enter_link(words);
    }

    fn leave_link(&mut self) {
        self.plain.leave_link();
    }

    fn enter_link_res(&mut self, resource: i64) {
        self.plain.enter_link_res(resource);
    }

    fn leave_link_res(&mut self) {
        self.plain.leave_link_res();
    }

    fn enter_self_link(&mut self) {
        self.plain.enter_self_link();
    }

    fn leave_self_link(&mut self) {
        self.plain.leave_self_link();
    }

    fn embed_res(&mut self, resource: i64) {
        self.plain.embed_res(resource);
    }

    fn can_embed_res(&self, resource: i64) -> bool {
        self.plain.can_embed_res(resource)
    }

    fn progress(&mut self, amount: i64, total: i64) {
        self.plain.progress(amount, total);
    }

    /// Turn on the deprecated style bits (Aa-machine: SET_STYLE).
    fn set_style(&mut self, bits: i64) {
        self.wardrobe.styled(bits);
        self.fitted();
    }

    /// Turn off the deprecated style bits.
    fn reset_style(&mut self, bits: i64) {
        self.wardrobe.unstyled(bits);
        self.fitted();
    }

    /// Return to the default text style.
    fn unstyle(&mut self) {
        self.wardrobe.bared();
        self.fitted();
    }

    fn clear(&mut self) {
        self.plain.clear();
    }

    fn clear_all(&mut self) {
        self.plain.clear_all();
    }

    fn clear_status(&mut self) {
        self.plain.clear_status();
    }

    fn clear_links(&mut self) {
        self.plain.clear_links();
    }

    fn clear_old(&mut self) {
        self.plain.clear_old();
    }

    fn clear_div(&mut self) {
        self.plain.clear_div();
    }

    /// Return to the initial state, the spans' dresses dropped.
    ///
    /// The machine clears its div ledger without a leave call per
    /// div, so the whole stack drops here with it; the body dress
    /// stays, being the document's rather than any division's.
    fn leave_all(&mut self) {
        self.plain.leave_all();
        self.wardrobe.dropped();
        self.fitted();
    }

    fn sync(&mut self) {
        self.plain.sync();
    }

    fn script_on(&mut self) -> bool {
        self.plain.script_on()
    }

    fn script_off(&mut self) {
        self.plain.script_off();
    }

    fn script_active(&self) -> bool {
        self.plain.script_active()
    }

    fn reset(&mut self) {
        self.plain.reset();
    }

    fn measured(&self, dimension: i64) -> i64 {
        self.plain.measured(dimension)
    }

    fn trace(&mut self, text: &str) {
        self.plain.trace(text);
    }

    /// Ask where to keep the savefile; an empty answer cancels.
    fn save(&mut self, data: &[u8]) -> bool {
        let name = self.named("Save the story as: ");

        if name.is_empty() {
            return false;
        }

        std::fs::write(name, data).is_ok()
    }

    /// Ask which savefile to revive; an empty answer cancels.
    fn restore(&mut self) -> Option<Vec<u8>> {
        let name = self.named("Restore the story from: ");

        if name.is_empty() {
            return None;
        }

        std::fs::read(name).ok()
    }
}

/// Play one story at the terminal, opening to quit.
///
/// The seams are explicit where the reference defaults them: a
/// source answers one raw input line at a time (None at the end),
/// the writer takes the poured story, and asked answers the
/// filename prompts. Dressed is the honesty gate for the LOOK
/// styles -- the CLI asks its own stream whether it is a real
/// terminal, and a pipe stays plain.
pub fn played(
    story: Story,
    seed: Option<u32>,
    mut source: Box<dyn FnMut() -> Option<String>>,
    writer: Box<dyn Write>,
    asked: Asked,
    width: i64,
    dressed: bool,
) -> Result<(), VoxamError> {
    let voice = TerminalVoice::new(&story, width, writer, asked, dressed)?;
    let mut machine = Machine::new(story, voice, seed)?;
    let mut waiting = machine.run(None)?;

    while waiting != Wait::Quit {
        machine.voice.poured();

        let Some(line) = source() else {
            machine.voice.line();

            break;
        };
        let line = line.trim_end_matches(['\r', '\n']).to_string();

        if waiting == Wait::Line {
            machine.voice.prompted();
            waiting = machine.deliver_line(&line)?;
        } else {
            let mut keys = line.chars();

            while waiting == Wait::Key {
                let Some(key) = keys.next() else {
                    break;
                };

                waiting = machine.deliver_key(u32::from(key))?;
            }

            if waiting == Wait::Key {
                waiting = machine.deliver_key(0x0D)?;
            }
        }
    }

    machine.voice.line();
    machine.voice.undressed();
    machine.voice.poured();

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::aamachine::story::{SUMMED, crc32};
    use crate::iff::chunk as iff_chunk;

    const ESC: &str = "\x1b";
    const QUIT: &[u8] = &[0x70, 0x00];

    // The LOOK sheet the dress tests wear: bold, italic, the
    // normal!important override, a red-and-bold class, and a
    // green-on-black italic body -- the same shapes Miss
    // Gosling's own sheets exercise.
    fn look() -> Vec<u8> {
        let classes: &[&[&str]] = &[
            &["font-weight: bold"],
            &["font-style: italic"],
            &["font-style: normal !important"],
            &["color: red", "font-weight: bold"],
            &[
                "color: green",
                "background-color: black",
                "font-style: italic",
            ],
        ];
        let mut offsets = Vec::new();
        let mut definitions = Vec::new();
        let base = 2 + classes.len() * 2;

        for class in classes {
            offsets.push((base + definitions.len()) as u16);

            for pair in *class {
                definitions.extend_from_slice(pair.as_bytes());
                definitions.push(0);
            }

            definitions.push(0);
        }

        let mut told = (classes.len() as u16).to_be_bytes().to_vec();

        for offset in offsets {
            told.extend_from_slice(&offset.to_be_bytes());
        }

        told.extend(definitions);

        told
    }

    // A minimal LANG: the four offsets, an empty extended table,
    // an empty endings table, and the three special sets.
    fn lang() -> Vec<u8> {
        let mut told = Vec::new();

        for offset in [8u16, 8, 9, 10] {
            told.extend_from_slice(&offset.to_be_bytes());
        }

        told.extend_from_slice(&[0, 0, 0, 0, 0, 0]);

        told
    }

    // A story around a code body, wearing the dress-test LOOK.
    fn storied(code: &[u8]) -> Story {
        let mut whole = vec![0x01];

        whole.extend_from_slice(code);

        let mut init = vec![0u8, 0, 0, 1, 0, 1];

        init.extend_from_slice(&[0, 1]);

        let summed = |name: &[u8; 4]| -> Vec<u8> {
            match name {
                b"LANG" => lang(),
                b"DICT" => vec![0, 0],
                b"MAPS" => vec![0, 0],
                b"LOOK" => look(),
                b"WRIT" => vec![0x80],
                b"INIT" => init.clone(),
                b"CODE" => whole.clone(),
                _ => Vec::new(),
            }
        };

        let mut crc = 0;

        for name in &SUMMED {
            crc = crc32(&summed(name), crc);
        }

        let mut head = vec![0, 5, 2, 0];

        head.extend_from_slice(&1u16.to_be_bytes());
        head.extend_from_slice(b"260827");
        head.extend_from_slice(&crc.to_be_bytes());
        head.extend_from_slice(&32u16.to_be_bytes());
        head.extend_from_slice(&16u16.to_be_bytes());
        head.extend_from_slice(&16u16.to_be_bytes());

        let mut pieces = iff_chunk(b"HEAD", &head);

        for name in &SUMMED {
            pieces.extend(iff_chunk(name, &summed(name)));
        }

        let mut body = b"AAVM".to_vec();

        body.extend(pieces);

        Story::new(&iff_chunk(b"FORM", &body)).unwrap()
    }

    // A writer the test can read back after the voice owns it.
    #[derive(Clone, Default)]
    struct Shared(Rc<RefCell<Vec<u8>>>);

    impl Shared {
        fn held(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).unwrap()
        }
    }

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);

            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn dressed_voice() -> TerminalVoice {
        TerminalVoice::new(
            &storied(QUIT),
            80,
            Box::new(Shared::default()),
            Box::new(|_prompt| String::new()),
            true,
        )
        .unwrap()
    }

    // Play a crafted story with scripted streams; the output
    // comes back.
    fn acted(code: &[u8], commands: &str, answers: &[&str]) -> String {
        let mut feed: Vec<String> = commands.lines().map(str::to_string).collect();

        feed.reverse();

        let mut asked: Vec<String> = answers.iter().map(|held| held.to_string()).collect();

        asked.reverse();

        let writer = Shared::default();

        played(
            storied(code),
            Some(7),
            Box::new(move || feed.pop()),
            Box::new(writer.clone()),
            Box::new(move |_prompt| asked.pop().unwrap_or_default()),
            80,
            false,
        )
        .unwrap();

        writer.held()
    }

    // A story that saves, restores, and reports which path ran --
    // the machine battery's own shape, here through real files.
    fn saving_code() -> Vec<u8> {
        let landing = 1 + 1 + 3 + 3 + 3 + QUIT.len() + 2;
        let mut body = vec![0x72];

        body.extend_from_slice(&[
            0x80 | (landing >> 16) as u8,
            (landing >> 8) as u8,
            landing as u8,
        ]);
        body.extend_from_slice(&[0x65, 0x40, 0x01]);
        body.extend_from_slice(&[0x70, 0x02]);
        body.extend_from_slice(&[0x65, 0x40, 0x03]);
        body.extend_from_slice(QUIT);
        body.extend_from_slice(&[0x65, 0x40, 0x02]);
        body.extend_from_slice(QUIT);

        body
    }

    fn keep_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("voxam-aaterminal-{name}"));

        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        dir
    }

    // A script that runs dry ends the session on a broken line.
    #[test]
    fn a_dry_script_ends_the_session() {
        let told = acted(&[0x65, 0x40, 0x05, 0x73, 0x00, 0x70, 0x00], "", &[]);

        assert!(told.contains('5'), "{told:?}");
        assert!(told.ends_with('\n'), "{told:?}");
    }

    // A key wait takes the line a keypress at a time, return
    // closing an exhausted line.
    #[test]
    fn key_waits_take_lines_as_keypresses() {
        let told = acted(&[0xF3, 0x00, 0x65, 0x80, 0x00, 0x70, 0x00], "q\n", &[]);

        assert!(told.contains('q'), "{told:?}");
    }

    // A save and restore round-trip through real files: the save
    // prints 1, the restore lands at the saved address to print 2.
    #[test]
    fn saves_round_trip_through_files() {
        let dir = keep_dir("round-trip");
        let keep = dir.join("kept").to_string_lossy().to_string();
        let told = acted(&saving_code(), "", &[&keep, &keep]);

        assert!(dir.join("kept.aasave").exists());
        assert!(
            told.contains('1') && told.contains('2') && !told.contains('3'),
            "{told:?}"
        );
    }

    // An empty filename cancels a save, and the story hears the
    // refusal: the failed save falls to the restore, which also
    // cancels, printing 3.
    #[test]
    fn an_empty_name_cancels_save_and_restore() {
        let mut code = vec![0x0A, 0x00, 0x80, 0x00, 0x0D];

        code.extend(saving_code());

        let told = acted(&code, "", &["", ""]);

        assert!(told.contains('3'), "{told:?}");
    }

    // A save that cannot be written, and a restore that cannot be
    // read, both land as polite failures.
    #[test]
    fn unwritable_and_unreadable_files_fail_politely() {
        let dir = keep_dir("nowhere");
        let nowhere = dir
            .join("no")
            .join("such")
            .join("file.aasave")
            .to_string_lossy()
            .to_string();
        let absent = dir.join("absent.aasave").to_string_lossy().to_string();
        let mut code = vec![0x0A, 0x00, 0x80, 0x00, 0x0D];

        code.extend(saving_code());

        let told = acted(&code, "", &[&nowhere, &absent]);

        assert!(told.contains('3'), "{told:?}");
    }

    // A dotted name is honored whole; only a bare one gains the
    // suffix.
    #[test]
    fn a_dotted_name_keeps_its_own_suffix() {
        let dir = keep_dir("dotted");
        let keep = dir.join("game.sav").to_string_lossy().to_string();

        acted(&saving_code(), "", &[&keep, &keep]);

        assert!(dir.join("game.sav").exists());
    }

    // The terminal voice's file manners stand alone: a bare name
    // gains the suffix through the asked seam.
    #[test]
    fn the_voice_asks_for_names() {
        let dir = keep_dir("manners");
        let held = dir.join("held").to_string_lossy().to_string();
        let story = storied(QUIT);
        let mut voice = TerminalVoice::new(
            &story,
            80,
            Box::new(Shared::default()),
            Box::new(move |_prompt| held.clone()),
            false,
        )
        .unwrap();

        assert!(voice.save(b"data"));
        assert!(dir.join("held.aasave").exists());
        assert_eq!(voice.restore(), Some(b"data".to_vec()));
    }

    // The default prompt text pours through the writer before the
    // question is asked.
    #[test]
    fn the_prompt_pours_the_pending_story_first() {
        let writer = Shared::default();
        let story = storied(QUIT);
        let mut voice = TerminalVoice::new(
            &story,
            80,
            Box::new(writer.clone()),
            Box::new(|_prompt| String::new()),
            false,
        )
        .unwrap();

        voice.say("pending");
        voice.save(b"data");

        assert!(writer.held().contains("pending"), "{:?}", writer.held());
    }

    // -- the dress ---------------------------------------------------------

    // A bold class lands as the terminal's own bold, and leaving
    // the span drops it.
    #[test]
    fn a_bold_span_wears_bold() {
        let mut voice = dressed_voice();

        voice.enter_span(0);
        voice.say("clue");
        voice.leave_span();

        assert_eq!(voice.told(), format!("{ESC}[0;1mclue{ESC}[0m"));
    }

    // Italics wear underlines, the Dialog debugger's own
    // rendering, and a nested normal!important turns them off --
    // Miss Gosling's sheets do exactly this inside italic
    // quotations.
    #[test]
    fn italics_wear_underlines_and_normal_overrides() {
        let mut voice = dressed_voice();

        voice.enter_span(1);
        voice.say("a");
        voice.enter_span(2);
        voice.say("b");
        voice.leave_span();
        voice.say("c");
        voice.leave_span();

        assert_eq!(
            voice.told(),
            format!("{ESC}[0;4ma{ESC}[0mb{ESC}[0;4mc{ESC}[0m")
        );
    }

    // A named color rides as truecolor ink, beside its bold.
    #[test]
    fn a_named_color_wears_truecolor() {
        let mut voice = dressed_voice();

        voice.enter_span(3);

        assert!(
            voice.told().contains(&format!("{ESC}[0;1;38;2;205;49;49m")),
            "{:?}",
            voice.plain.told
        );
    }

    // The body dress layers beneath everything: green ink on
    // black paper, in italics-as-underline, exactly its sheet.
    #[test]
    fn the_body_wears_its_whole_sheet() {
        let mut voice = dressed_voice();

        voice.set_body(4);

        assert_eq!(
            voice.told(),
            format!("{ESC}[0;4;38;2;13;188;121;48;2;0;0;0m")
        );
    }

    // The deprecated style bits compose and clear: bold and
    // reverse on, bold off leaves reverse, unstyle drops the rest.
    #[test]
    fn the_deprecated_bits_compose() {
        let mut voice = dressed_voice();

        voice.set_style(3);
        voice.reset_style(2);
        voice.unstyle();

        assert_eq!(voice.told(), format!("{ESC}[0;1;7m{ESC}[0;7m{ESC}[0m"));
    }

    // leave_all drops every open span's dress at once -- the
    // machine clears its ledger without a leave call per div.
    #[test]
    fn leave_all_drops_the_stack() {
        let mut voice = dressed_voice();

        voice.enter_span(0);
        voice.enter_span(1);
        voice.leave_all();

        assert!(voice.told().ends_with(&format!("{ESC}[0m")));
    }

    // A class LOOK never named wears the bare dress, and divs
    // carry their dress around their breaks.
    #[test]
    fn unnamed_classes_and_divs_dress_too() {
        let mut voice = dressed_voice();

        voice.enter_div(0);
        voice.say("title");
        voice.leave_div(0);
        voice.enter_span(99);

        let told = voice.told().to_string();

        assert!(told.contains(&format!("{ESC}[0;1mtitle")), "{told:?}");
        assert!(told.ends_with(&format!("{ESC}[0m")), "{told:?}");
    }

    // Inside a hidden status area no dress lands at all.
    #[test]
    fn a_hidden_status_swallows_the_dress() {
        let mut voice = dressed_voice();

        voice.enter_status(0, 0);
        voice.enter_span(0);
        voice.say("hidden");
        voice.leave_span();
        voice.leave_status();

        assert!(!voice.told().contains(ESC), "{:?}", voice.plain.told);
    }

    // An undressed voice stays plain everywhere and reports
    // itself honestly to VM_INFO's seams.
    #[test]
    fn the_honesty_gate_holds() {
        let story = storied(QUIT);
        let mut plain = TerminalVoice::new(
            &story,
            80,
            Box::new(Shared::default()),
            Box::new(|_prompt| String::new()),
            false,
        )
        .unwrap();

        plain.enter_span(0);
        plain.say("plain");
        plain.undressed();

        assert_eq!(plain.told(), "plain");
        assert!(!plain.has_styles());
        assert!(!plain.has_color());

        let worn = dressed_voice();

        assert!(worn.has_styles());
        assert!(worn.has_color());
        assert!(worn.has_saves());
    }

    // A whole dressed session takes every attribute off at the
    // end.
    #[test]
    fn a_dressed_session_closes_clean() {
        let writer = Shared::default();

        played(
            storied(QUIT),
            Some(7),
            Box::new(|| None),
            Box::new(writer.clone()),
            Box::new(|_prompt| String::new()),
            80,
            true,
        )
        .unwrap();

        assert!(
            writer.held().ends_with(&format!("{ESC}[0m")),
            "{:?}",
            writer.held()
        );
    }

    // The color parser speaks hex short and long, rgb(), and
    // shrugs at what it cannot mix; the weight parser hears
    // normal too.
    #[test]
    fn the_color_and_weight_parsers() {
        use crate::aamachine::output::{Dress, tinted};

        assert_eq!(tinted("#fff"), Some((255, 255, 255)));
        assert_eq!(tinted("#a1b2c3"), Some((161, 178, 195)));
        assert_eq!(tinted("rgb(1, 2, 3)"), Some((1, 2, 3)));
        assert_eq!(tinted("rgb(bad)"), None);
        assert_eq!(tinted("rgb(1, 2, x)"), None);
        assert_eq!(tinted("linen"), None);
        assert_eq!(tinted(""), None);

        let pairs = vec![
            ("font-weight".to_string(), "normal".to_string()),
            ("font-style".to_string(), "oblique".to_string()),
        ];
        let dress = Dress::new(&pairs);

        assert_eq!(dress.bold, Some(false));
        assert_eq!(dress.italic, Some(true));
    }

    // A leave with nothing worn is a story's imbalance, answered
    // calmly with the bare dress rather than a crash.
    #[test]
    fn an_unworn_leave_stays_calm() {
        let mut voice = dressed_voice();

        voice.leave_span();
        voice.leave_div(0);

        assert!(voice.told().starts_with(&format!("{ESC}[0m{ESC}[0m")));
    }
}
