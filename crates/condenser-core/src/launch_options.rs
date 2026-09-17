//! Steam-compatible launch options.
//!
//! Mirrors the semantics of the Steam client's per-game "Launch Options" field, so the
//! strings users already have can be pasted straight across:
//!
//! ```text
//! gamescope --hdr-enabled -W 3440 -H 1440 -r 180 -f -- %command%
//! WINEDLLOVERRIDES="mss32=n,b" %command% -novid
//! -windowed -w 1920
//! ```
//!
//! Rules:
//! * `%command%` stands for the command Condenser would otherwise run (`umu-run <exe>`).
//! * Tokens before it form a **wrapper** — the first is the program to exec, the rest
//!   are its arguments. `VAR=value` pairs at the front become environment variables.
//! * Tokens after it are appended as extra arguments to the game.
//! * With no `%command%` at all, the whole string is treated as extra game arguments,
//!   which is what Steam does.

/// The placeholder Steam uses for the real command line.
pub const PLACEHOLDER: &str = "%command%";

/// A launch-options string broken into the parts a launcher needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    /// `VAR=value` assignments that preceded the wrapper program.
    pub env: Vec<(String, String)>,
    /// Program and arguments that should wrap the real command.
    pub wrapper: Vec<String>,
    /// Arguments appended after the real command.
    pub extra_args: Vec<String>,
}

impl LaunchOptions {
    /// Nothing to apply — plain launch.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.env.is_empty() && self.wrapper.is_empty() && self.extra_args.is_empty()
    }
}

/// Parse a Steam-style launch options string.
///
/// Never fails: an unterminated quote simply ends at the end of input, matching how
/// shells and Steam itself behave in practice rather than rejecting the user's input.
#[must_use]
pub fn parse(input: &str) -> LaunchOptions {
    let tokens = tokenize(input);
    let Some(split) = tokens.iter().position(|t| t == PLACEHOLDER) else {
        // No placeholder: everything is extra arguments for the game.
        return LaunchOptions {
            extra_args: tokens,
            ..LaunchOptions::default()
        };
    };

    let (before, after) = tokens.split_at(split);
    let mut env = Vec::new();
    let mut wrapper: Vec<String> = Vec::new();

    for token in before {
        // Leading VAR=value pairs are environment, but only until the wrapper program
        // starts — after that an `=` is just part of an argument.
        if wrapper.is_empty() {
            if let Some((key, value)) = split_assignment(token) {
                env.push((key, value));
                continue;
            }
        }
        wrapper.push(token.clone());
    }

    LaunchOptions {
        env,
        wrapper,
        // Skip the placeholder itself.
        extra_args: after.iter().skip(1).cloned().collect(),
    }
}

/// Split `KEY=value` when it looks like a shell assignment.
fn split_assignment(token: &str) -> Option<(String, String)> {
    let (key, value) = token.split_once('=')?;
    if key.is_empty() {
        return None;
    }
    let valid = key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !key.starts_with(|c: char| c.is_ascii_digit());
    valid.then(|| (key.to_owned(), value.to_owned()))
}

/// Shell-like tokenizer: splits on whitespace, honours single and double quotes and
/// backslash escapes. Quotes are removed, so `A="b c"` becomes the token `A=b c`.
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = input.chars();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    // Inside double quotes a backslash escapes the next character.
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                        started = true;
                    }
                }
                c if c.is_whitespace() => {
                    if started {
                        tokens.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                c => {
                    current.push(c);
                    started = true;
                }
            },
        }
    }
    if started {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(parse("").is_empty());
        assert!(parse("   ").is_empty());
    }

    /// A real `gamescope` line copied from the Steam client.
    #[test]
    fn gamescope_wrapper_is_parsed() {
        let opts = parse("gamescope --hdr-enabled -W 3440 -H 1440 -r 180 -f -- %command%");
        assert_eq!(
            opts.wrapper,
            s(&[
                "gamescope",
                "--hdr-enabled",
                "-W",
                "3440",
                "-H",
                "1440",
                "-r",
                "180",
                "-f",
                "--"
            ])
        );
        assert!(opts.env.is_empty());
        assert!(opts.extra_args.is_empty());
    }

    /// The `CoD4` audio fix: an environment assignment plus trailing game arguments.
    #[test]
    fn env_assignment_and_trailing_args() {
        let opts = parse(r#"WINEDLLOVERRIDES="mss32=n,b" %command% -novid -windowed"#);
        assert_eq!(
            opts.env,
            vec![("WINEDLLOVERRIDES".to_owned(), "mss32=n,b".to_owned())]
        );
        assert!(opts.wrapper.is_empty());
        assert_eq!(opts.extra_args, s(&["-novid", "-windowed"]));
    }

    /// Steam treats a string with no placeholder as plain game arguments.
    #[test]
    fn without_placeholder_everything_is_game_args() {
        let opts = parse("-windowed -w 1920 -h 1080");
        assert!(opts.wrapper.is_empty());
        assert!(opts.env.is_empty());
        assert_eq!(opts.extra_args, s(&["-windowed", "-w", "1920", "-h", "1080"]));
    }

    #[test]
    fn multiple_env_vars_before_wrapper() {
        let opts = parse("DXVK_HUD=fps MANGOHUD=1 mangohud %command%");
        assert_eq!(
            opts.env,
            vec![
                ("DXVK_HUD".to_owned(), "fps".to_owned()),
                ("MANGOHUD".to_owned(), "1".to_owned()),
            ]
        );
        assert_eq!(opts.wrapper, s(&["mangohud"]));
    }

    /// An `=` after the wrapper program has begun is an argument, not an assignment.
    #[test]
    fn equals_after_wrapper_start_is_an_argument() {
        let opts = parse("gamescope --backend=sdl %command%");
        assert_eq!(opts.wrapper, s(&["gamescope", "--backend=sdl"]));
        assert!(opts.env.is_empty());
    }

    #[test]
    fn quotes_and_escapes() {
        assert_eq!(tokenize(r#"a "b c" 'd e' f\ g"#), s(&["a", "b c", "d e", "f g"]));
        assert_eq!(tokenize(r#""" x"#), s(&["", "x"]), "empty quoted token is kept");
        assert_eq!(tokenize(r#"path="/a b/c""#), s(&["path=/a b/c"]));
        // Unterminated quote consumes the rest rather than erroring.
        assert_eq!(tokenize(r#"a "b c"#), s(&["a", "b c"]));
    }

    #[test]
    fn digits_and_empty_keys_are_not_assignments() {
        assert_eq!(split_assignment("1BAD=x"), None);
        assert_eq!(split_assignment("=x"), None);
        assert_eq!(split_assignment("-flag=x"), None);
        assert_eq!(
            split_assignment("GOOD_1=x"),
            Some(("GOOD_1".to_owned(), "x".to_owned()))
        );
    }
}
