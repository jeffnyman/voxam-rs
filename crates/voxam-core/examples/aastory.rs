//! Print an Å-machine story's report: the header's claims, the
//! bibliography, the extended character table, the chunk census,
//! and the whole dictionary decoded. The Python reference prints
//! the same form; the two must agree line for line.

use voxam_core::aamachine::story::Story;
use voxam_core::aamachine::text::Speech;

fn main() {
    let path = std::env::args().nth(1).expect("usage: aastory <story>");
    let data = std::fs::read(&path).expect("readable story");

    let story = match Story::new(&data) {
        Ok(story) => story,
        Err(error) => {
            println!("REFUSED: {error}");
            return;
        }
    };

    println!(
        "version={}.{} wordsz={} shift={} release={} serial={} checksum={:08x} \
         heap={} aux={} ram={}",
        story.version.0,
        story.version.1,
        story.word_size,
        story.shift,
        story.release,
        story.serial,
        story.checksum,
        story.heap_size,
        story.aux_size,
        story.ram_size,
    );
    println!("ifid={}", story.ifid.as_deref().unwrap_or("-"));

    for (name, value) in &story.meta {
        println!("meta {name}={value}");
    }

    let extended: String = story.extended.iter().collect();

    println!("extended={}:{extended}", story.extended.len());

    let census: Vec<String> = story
        .chunks
        .iter()
        .map(|held| {
            format!(
                "{}:{}",
                held.chunk_id.iter().map(|&b| b as char).collect::<String>(),
                held.payload.len()
            )
        })
        .collect();

    println!("chunks={}", census.join(","));
    println!("files={}", story.files().count());

    match Speech::new(&story) {
        Err(error) => println!("REFUSED: {error}"),
        Ok(speech) => {
            println!("words={}", speech.words.len());

            for (seat, word) in speech.words.iter().enumerate() {
                println!("{seat}: {word}");
            }
        }
    }
}
