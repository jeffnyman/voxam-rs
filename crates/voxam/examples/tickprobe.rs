//! A scratch probe: Border Zone's ticking read, watched up close.
//!
//! Drives chapter 1 on a TestBackend glass, lets real deadlines
//! expire so deliver_tick fires, and reports what each tick did:
//! whether the interrupt printed, where, and what the model's
//! screen holds. Not a battery -- a magnifying glass.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use voxam_core::zmachine::machine::{Identity, Machine, RunState, Waiting, Wants};
use voxam_core::zmachine::story::Story;
use voxam_glass::keys::ScriptedKeys;
use voxam_glass::painter::{Glass, PaintedHalf, ScreenFrontend};
use voxam_glass::ratatui::Terminal;
use voxam_glass::ratatui::backend::TestBackend;

fn main() {
    let bytes = std::fs::read("entharion/zcode-infocom/borderzone-r9-s871008.z5")
        .expect("the corpus story");
    let story = Story::new(bytes).expect("a loadable story");
    let terminal = Terminal::new(TestBackend::new(80, 24)).expect("a test terminal");
    let glass = Rc::new(RefCell::new(Glass::new(
        terminal,
        Box::new(ScriptedKeys::new(vec![Some(' '); 60])),
        Box::new(|| ()),
    )));
    let face = Rc::new(RefCell::new(ScreenFrontend::new(5, glass)));
    let mut machine = Machine::new(
        story,
        Box::new(PaintedHalf(Rc::clone(&face))),
        Some(1),
        Identity::default(),
        None,
    )
    .expect("a bootable machine");

    let mut ticks = 0;
    let mut keyed = false;
    let mut hastened = false;
    let mut walked = 0;
    let chapter = std::env::var("PROBE_CHAPTER").unwrap_or_else(|_| "1".to_string());
    let chapter = chapter.chars().next().unwrap_or('1');

    loop {
        match machine.run().expect("a running machine") {
            RunState::Halted => {
                println!("HALTED");
                break;
            }
            RunState::Waiting => {}
        }

        let (wants, time, routine) = match machine.waiting() {
            Some(Waiting::Read(reading)) => (reading.wants, reading.time, reading.routine),
            _ => {
                println!("a file wait?!");
                break;
            }
        };

        println!("WAIT: {wants:?} time={time} routine={routine:#x}");

        if !keyed {
            // The chapter menu: pick chapter 1.
            if wants == Wants::Key {
                machine.deliver_key(chapter).expect("a delivered key");
                keyed = true;

                continue;
            }

            machine.deliver_line("", 0).expect("a delivered line");

            continue;
        }

        if time == 0 || routine == 0 {
            println!("an untimed read after the chapter menu -- stopping");
            break;
        }

        // What does FAST do to the cadence? Ask once, then watch.
        if std::env::var("PROBE_FAST").is_ok() && !hastened {
            hastened = true;
            machine.deliver_line("fast", 0).expect("a delivered FAST");

            continue;
        }

        // Walk somewhere first: the planned scenes are keyed to
        // where the player stands, not just the hour.
        if let Ok(walk) = std::env::var("PROBE_WALK")
            && walked < walk.split(',').count()
        {
            let command = walk.split(',').nth(walked).expect("a scripted command");

            println!("> {command}");
            walked += 1;
            machine
                .deliver_line(command, 0)
                .expect("a delivered command");

            continue;
        }

        // The machine knows no wall clock: 1250 instant ticks are
        // 1250 game seconds, minute flips and planned events
        // included.
        let _ = Duration::ZERO;

        loop {
            face.borrow_mut().begin_input();

            let serial = machine.wait_serial();
            let before = face.borrow().prints;
            let table = machine
                .memory_mut()
                .read_word(0x0C)
                .expect("the globals table address") as usize;
            let held: Vec<u16> = (0..240)
                .map(|index| {
                    machine
                        .memory_mut()
                        .read_word(table + index * 2)
                        .expect("a global")
                })
                .collect();

            machine.deliver_tick().expect("a delivered tick");

            for (index, was) in held.iter().enumerate() {
                let now = machine
                    .memory_mut()
                    .read_word(table + index * 2)
                    .expect("a global");

                if now != *was {
                    println!("  global {index}: {was} -> {now}");
                }
            }

            let after = face.borrow().prints;

            if machine.waiting().is_some() && machine.wait_serial() != serial {
                println!("== tick {ticks}: A READ INSIDE THE INTERRUPT; the scene: ==");

                {
                    let mut held = face.borrow_mut();

                    for row in 1..=24 {
                        let text = held.model.row_text(row);

                        if !text.trim().is_empty() {
                            println!("  {row:2}|{text}");
                        }
                    }
                }

                machine.deliver_line("", 0).expect("an answered scene");

                match machine.run().expect("a climb out of the interrupt") {
                    RunState::Halted => {
                        println!("HALTED inside the scene");
                        return;
                    }
                    RunState::Waiting => {
                        println!(
                            "== answered; the outer read stands again: {:?} ==",
                            matches!(machine.waiting(), Some(Waiting::Read(_)))
                        );
                    }
                }
            }

            let standing = machine.waiting().is_some();

            {
                let mut held = face.borrow_mut();
                let status = held.model.row_text(1);
                let clock = status.trim().rsplit(' ').next().unwrap_or("").to_string();

                println!(
                    "tick {ticks}: prints {before}->{after}, standing={standing}, clock={clock}"
                );
            }

            if after != before {
                face.borrow_mut().resume_input();
                println!("  the interrupt printed to the story window");

                if std::env::var("PROBE_DUMP").is_ok() {
                    let mut held = face.borrow_mut();

                    for row in 1..=24 {
                        let text = held.model.row_text(row);

                        if !text.trim().is_empty() {
                            println!("  {row:2}|{text}");
                        }
                    }

                    return;
                }
            }

            if !standing {
                println!("  the interrupt terminated the read");
                face.borrow_mut().abandon_input();

                break;
            }

            ticks += 1;

            if ticks >= 1250 {
                println!("-- 1250 ticks served; the model's screen: --");

                let mut held = face.borrow_mut();

                for row in 1..=24 {
                    let text = held.model.row_text(row);

                    if !text.trim().is_empty() {
                        println!("{row:2}|{text}");
                    }
                }

                return;
            }
        }
    }
}
