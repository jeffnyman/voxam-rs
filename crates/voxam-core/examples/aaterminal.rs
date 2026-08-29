//! Play an Å-machine story through the terminal face with
//! scripted streams: the input lines come from a script file, the
//! filename prompts always cancel, and the session stays
//! undressed. The certify sweep runs the same drill under the
//! Python reference and diffs the two sessions whole.
//!
//! Usage: aaterminal <story> [script]

use voxam_core::aamachine::story::Story;
use voxam_core::aamachine::terminal::played;

fn main() {
    let mut args = std::env::args().skip(1);
    let story_path = args.next().expect("usage: aaterminal <story> [script]");
    let data = std::fs::read(&story_path).expect("readable story");
    let story = Story::new(&data).expect("a story file");
    let script = args
        .next()
        .map(|path| {
            String::from_utf8(std::fs::read(&path).expect("readable script"))
                .expect("a UTF-8 script")
        })
        .unwrap_or_default();

    let mut feed: Vec<String> = script.lines().map(str::to_string).collect();

    feed.reverse();

    played(
        story,
        Some(7),
        Box::new(move || feed.pop()),
        Box::new(std::io::stdout()),
        Box::new(|_prompt| String::new()),
        80,
        false,
    )
    .expect("a clean session");
}
