//! Name and verify Glulx story files: the loader's smoke test.

fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("readable story");

        match voxam_core::glulx::story::Story::new(bytes) {
            Ok(story) => println!(
                "{}: Glulx {}, checksum {}",
                std::path::Path::new(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                story.version(),
                if story.verify() {
                    "verified"
                } else {
                    "MISMATCH"
                }
            ),
            Err(error) => println!("{path}: {error}"),
        }
    }
}
