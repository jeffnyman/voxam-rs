//! Acceptance scripts: recorded sessions the CLI can replay.
//!
//! A script is a plain text file of typed commands plus a few
//! directives, most importantly which game to run and which seed
//! to roll with -- the grammar of the Python reference's
//! acceptance module, ported for the plain-stream replays this
//! machine can already carry. The special keys, clicks, and link
//! selections wait for the keystroke seam; a script that presses
//! them reports the frontier by name.

use std::path::{Path, PathBuf};

/// One recorded session: a game, a seed, and the typed commands,
/// each with its line number in the script file.
pub struct AcceptanceScript {
    pub game: PathBuf,
    pub seed: Option<u32>,
    pub commands: Vec<(String, usize)>,
}

/// The parser's refusal dialect: responses meaning a typed command
/// did not do what it said, curated from the Infocom house parser
/// and the Inform library -- the reference's own list, entry for
/// entry, each one earned by an observed refusal.
const REFUSAL_OPENINGS: [&str; 22] = [
    "I beg your pardon",
    "I didn't understand that sentence",
    "I don't know the word",
    "I only understood you as far as",
    "It's not clear what you're referring to",
    "Nice try",
    "That sentence isn't one I recognize",
    "That's not a verb I recogni",
    "There was no verb in that sentence",
    "What do you want",
    "You are not holding",
    "You aren't holding that",
    "You can't be serious",
    "You can't do that",
    "You can't go that way",
    "You can't quite reach",
    "You can't see any",
    "You must use a verb",
    "You should close it first",
    "You should open it first",
    "You're holding too many",
    "Your load is too heavy",
];

/// Disambiguation questions bury their tell mid-line, so this
/// family is sought anywhere in the line.
const REFUSAL_TELLS: [&str; 1] = ["do you mean"];

/// Find the first line of a response spoken in the refusal
/// dialect, stripped, or None when the response contains none.
pub fn refusal_in(response: &str) -> Option<String> {
    for line in response.lines() {
        let candidate = line.trim();

        // AMFV brackets its parser messages, so the anchor looks
        // past a leading bracket.
        let lowered = candidate.to_lowercase();
        let lowered = lowered.strip_prefix('[').unwrap_or(&lowered);

        let opens = REFUSAL_OPENINGS
            .iter()
            .any(|opening| lowered.starts_with(&opening.to_lowercase()));
        let tells = REFUSAL_TELLS.iter().any(|tell| lowered.contains(tell));

        if opens || tells {
            return Some(candidate.to_string());
        }
    }

    None
}

/// A command in the reference warning's dress: Python's repr,
/// single quotes unless the command holds one.
pub fn shown(command: &str) -> String {
    if command.contains('\'') {
        format!("\"{command}\"")
    } else {
        format!("'{command}'")
    }
}

/// The special keys a recording can press, one line each: the
/// translated command is the key's own input character -- the
/// §3.8.4 cursor codes, the §3.8.2.6 escape, and the space bar,
/// which line-stripping would otherwise erase. The keystroke seam
/// spends each as a single press; the enter key stays what it
/// always was: a bare >.
const KEY_TOKENS: [(&str, &str); 6] = [
    ("<up>", "\u{81}"),
    ("<down>", "\u{82}"),
    ("<left>", "\u{83}"),
    ("<right>", "\u{84}"),
    ("<escape>", "\u{1b}"),
    ("<space>", " "),
];

/// What the replay transcript shows for a pressed key: the token,
/// never the raw control character.
pub fn echoed(command: &str) -> &str {
    for (token, character) in KEY_TOKENS {
        if command == character {
            return token;
        }
    }

    command
}

impl AcceptanceScript {
    /// Read an acceptance script file (the reference grammar):
    /// `! KEY=VALUE` directives, `#` comments, ``` fences, the
    /// optional `>` prompt, inline comments at whitespace + #.
    pub fn parse(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;

        let mut game: Option<PathBuf> = None;
        let mut seed: Option<u32> = None;
        let mut commands = Vec::new();
        let mut fenced = false;

        for (number, raw) in text.lines().enumerate() {
            let number = number + 1;
            let line = raw.trim();

            if line.starts_with("```") {
                fenced = !fenced;
                continue;
            }

            if fenced || line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some(directive) = line.strip_prefix('!') {
                let (key, value) = directive
                    .split_once('=')
                    .ok_or_else(|| format!("line {number}: malformed directive"))?;

                match key.trim() {
                    "SEED" => {
                        seed = Some(
                            value
                                .trim()
                                .parse()
                                .map_err(|_| format!("line {number}: unusable seed"))?,
                        );
                    }
                    "GAME" => {
                        let value = Path::new(value.trim());
                        game = Some(if value.is_absolute() {
                            value.to_path_buf()
                        } else {
                            path.parent().unwrap_or(Path::new(".")).join(value)
                        });
                    }
                    other => return Err(format!("line {number}: unknown directive {other}")),
                }

                continue;
            }

            if line.starts_with("<click ")
                || line.starts_with("<double-click ")
                || line.starts_with("<link ")
            {
                return Err(format!(
                    "line {number}: {line} needs the pointer seam, which has not \
                     arrived yet"
                ));
            }

            if let Some((_, character)) = KEY_TOKENS.iter().find(|(token, _)| *token == line) {
                commands.push((character.to_string(), number));
            } else {
                commands.push((command_of(line), number));
            }
        }

        let game = game.ok_or_else(|| {
            format!(
                "{} names no game; add '! GAME=<story file>'",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        })?;

        Ok(Self {
            game,
            seed,
            commands,
        })
    }
}

/// Reduce a command line to what the player would have typed: the
/// optional > prefix drops, and a command starting with # after
/// the prefix is taken verbatim -- the escape for the rare command
/// that begins with a marker character.
fn command_of(line: &str) -> String {
    if let Some(rest) = line.strip_prefix('>') {
        let rest = rest.trim_start();

        if rest.starts_with('#') {
            return rest.to_string();
        }

        return uncommented(rest);
    }

    uncommented(line)
}

/// Cut an inline comment: whitespace followed by #.
fn uncommented(line: &str) -> String {
    let mut previous_was_space = false;

    for (index, character) in line.char_indices() {
        if character == '#' && previous_was_space {
            let mut kept = &line[..index];
            kept = kept.trim_end();

            return kept.to_string();
        }

        previous_was_space = character.is_whitespace();
    }

    line.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_command_lines() {
        assert_eq!(command_of("> open mailbox"), "open mailbox");
        assert_eq!(command_of("open mailbox"), "open mailbox");
        assert_eq!(
            command_of("x me. x mailbox   # a comment"),
            "x me. x mailbox"
        );
        assert_eq!(command_of("> #literal"), "#literal");
        assert_eq!(command_of(">"), "");
    }
}
