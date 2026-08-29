//! Run a Glulx story under a scripted blocking frontend and print
//! everything its windows accumulated -- the first look at
//! glulxercise's verdict.

use voxam_core::glulx::glk::api::Glk;
use voxam_core::glulx::glk::frontend::{Asked, Frontend};
use voxam_core::glulx::glk::objects::{Window, WindowMap};
use voxam_core::glulx::machine::Machine;
use voxam_core::glulx::story::Story;

struct Scripted {
    lines: Vec<String>,
}

impl Frontend for Scripted {
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
        if self.lines.is_empty() {
            return Asked::End;
        }

        Asked::Answer((self.lines.remove(0), 0))
    }

    fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
        Asked::End
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: glulxercise_probe <story.ulx> [commands...]");
    let lines: Vec<String> = args.collect();

    let data = std::fs::read(&path).expect("a readable story");
    let mut machine = Machine::new(Story::new(data).unwrap(), Some(1)).unwrap();

    machine.install_glk(Glk::new(Box::new(Scripted { lines })));

    match machine.run(Some(200_000_000)) {
        Ok(steps) => eprintln!("[ran {steps} steps]"),
        Err(error) => eprintln!("[halted: {error}]"),
    }

    if let Some(glk) = machine.glk_mut() {
        let keys: Vec<u32> = glk.window_order.clone();

        for key in keys {
            let text = glk.windows.get_mut(&key).map(Window::take_text);

            if let Some(text) = text
                && !text.is_empty()
            {
                println!("{text}");
            }
        }
    }
}
