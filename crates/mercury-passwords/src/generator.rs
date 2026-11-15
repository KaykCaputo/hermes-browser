//! Password generation.
//!
//! A password is built in three steps:
//! 1. one character from each enabled class, so every class is present;
//! 2. the remaining characters from the union of the enabled classes;
//! 3. a Fisher–Yates shuffle, so the forced characters can be anywhere.
//!
//! Every choice — a character from a set of `n`, or a shuffle position from `i + 1` — is one byte
//! from the random source, rejected and redrawn when it is `256 - 256 % n` or more and otherwise
//! reduced modulo `n`. Accepted bytes are spread evenly over `0..n`, so no character and no
//! position is favoured (no modulo bias). Each set has at most 78 characters and each shuffle
//! range at most [`GENERATOR_MAX_LENGTH`] (128) positions, so one byte is always enough and a
//! byte is rejected with probability below one half.

use zeroize::Zeroizing;

use crate::{Password, PasswordError};

/// Lower-case letters.
pub const LOWERCASE: &str = "abcdefghijklmnopqrstuvwxyz";
/// Upper-case letters.
pub const UPPERCASE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
/// Digits.
pub const DIGITS: &str = "0123456789";
/// Symbols: ones that sign-up forms commonly accept. Left out are quotes, the backslash, the
/// grave accent, brackets of every kind, `/`, `,`, `;`, `|` and the space, which sites more often
/// refuse or which break shell, CSV and HTML contexts when a password is pasted.
pub const SYMBOLS: &str = "!#$%&*+-.:=?@^_~";
/// Characters left out when `avoid_ambiguous` is set: zero and the letter O in both cases, and
/// the digit one, upper-case I, lower-case l and the vertical bar, which many fonts draw alike.
pub const AMBIGUOUS_CHARACTERS: &str = "0Oo1Il|";
/// The shortest password generated.
pub const GENERATOR_MIN_LENGTH: usize = 8;
/// The longest password generated.
pub const GENERATOR_MAX_LENGTH: usize = 128;

/// A random byte this many times in a row unusable means the source is broken.
const MAX_REJECTIONS_PER_DRAW: usize = 1_000;

/// What to generate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratorOptions {
    /// Number of characters, from [`GENERATOR_MIN_LENGTH`] to [`GENERATOR_MAX_LENGTH`].
    pub length: usize,
    /// Use [`LOWERCASE`].
    pub lowercase: bool,
    /// Use [`UPPERCASE`].
    pub uppercase: bool,
    /// Use [`DIGITS`].
    pub digits: bool,
    /// Use [`SYMBOLS`].
    pub symbols: bool,
    /// Leave out [`AMBIGUOUS_CHARACTERS`].
    pub avoid_ambiguous: bool,
}

impl Default for GeneratorOptions {
    /// 20 characters, all four classes, ambiguous characters avoided.
    fn default() -> Self {
        Self {
            length: 20,
            lowercase: true,
            uppercase: true,
            digits: true,
            symbols: true,
            avoid_ambiguous: true,
        }
    }
}

/// A source of random bytes.
pub trait RandomSource {
    /// Fills `bytes` with random values.
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), PasswordError>;
}

/// The operating system's generator, through the `getrandom` crate (`getentropy` on macOS).
pub struct OsRandom;

impl RandomSource for OsRandom {
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), PasswordError> {
        getrandom::fill(bytes).map_err(|_| PasswordError::RandomUnavailable)
    }
}

/// Generates a password with `options`, drawing from `random`.
///
/// Errors: [`PasswordError::InvalidInput`] for a length outside
/// [`GENERATOR_MIN_LENGTH`]`..=`[`GENERATOR_MAX_LENGTH`] or when no class is enabled;
/// [`PasswordError::RandomUnavailable`] when the source fails or never yields a usable byte.
pub fn generate_password(
    options: &GeneratorOptions,
    random: &mut impl RandomSource,
) -> Result<Password, PasswordError> {
    let classes = classes(options)?;
    let union: Vec<u8> = classes.concat();
    let mut characters = Zeroizing::new(Vec::with_capacity(options.length));
    for class in &classes {
        characters.push(class[uniform(random, class.len())?]);
    }
    while characters.len() < options.length {
        characters.push(union[uniform(random, union.len())?]);
    }
    for index in (1..characters.len()).rev() {
        let other = uniform(random, index + 1)?;
        characters.swap(index, other);
    }
    let mut text = String::with_capacity(characters.len());
    text.extend(characters.iter().map(|&byte| char::from(byte)));
    Ok(Password::new(text))
}

/// Entropy of a password drawn with these options, in bits; `0.0` for options that cannot
/// generate.
///
/// This is the entropy of the draws before the shuffle: `log2 |class|` for each forced
/// character plus `log2 |union|` for each other character. It is a lower bound on the entropy of
/// the shuffled password, because the password determines the draws once the shuffle is known.
pub fn entropy_bits(options: &GeneratorOptions) -> f64 {
    let Ok(classes) = classes(options) else {
        return 0.0;
    };
    let union: usize = classes.iter().map(Vec::len).sum();
    let forced: f64 = classes
        .iter()
        .map(|class| (class.len() as f64).log2())
        .sum();
    forced + (options.length - classes.len()) as f64 * (union as f64).log2()
}

/// The enabled classes, ambiguous characters removed if asked, after checking the options.
fn classes(options: &GeneratorOptions) -> Result<Vec<Vec<u8>>, PasswordError> {
    if !(GENERATOR_MIN_LENGTH..=GENERATOR_MAX_LENGTH).contains(&options.length) {
        return Err(PasswordError::InvalidInput(
            "the password length is outside the supported range",
        ));
    }
    let classes: Vec<Vec<u8>> = [
        (options.lowercase, LOWERCASE),
        (options.uppercase, UPPERCASE),
        (options.digits, DIGITS),
        (options.symbols, SYMBOLS),
    ]
    .into_iter()
    .filter(|(enabled, _)| *enabled)
    .map(|(_, class)| {
        class
            .bytes()
            .filter(|byte| {
                !(options.avoid_ambiguous && AMBIGUOUS_CHARACTERS.as_bytes().contains(byte))
            })
            .collect()
    })
    .collect();
    if classes.is_empty() {
        return Err(PasswordError::InvalidInput("no character class is enabled"));
    }
    Ok(classes)
}

/// A uniformly distributed number in `0..bound`, for `1 <= bound <= 256`, by rejection sampling.
fn uniform(random: &mut impl RandomSource, bound: usize) -> Result<usize, PasswordError> {
    debug_assert!((1..=256).contains(&bound));
    let zone = 256 - 256 % bound;
    let mut byte = Zeroizing::new([0u8; 1]);
    for _ in 0..MAX_REJECTIONS_PER_DRAW {
        random.fill(&mut byte[..])?;
        let value = usize::from(byte[0]);
        if value < zone {
            return Ok(value % bound);
        }
    }
    Err(PasswordError::RandomUnavailable)
}
