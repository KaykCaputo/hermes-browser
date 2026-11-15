//! The password generator, mostly with deterministic random sources.

use std::collections::HashMap;

use mercury_passwords::{
    AMBIGUOUS_CHARACTERS, DIGITS, GENERATOR_MAX_LENGTH, GENERATOR_MIN_LENGTH, GeneratorOptions,
    LOWERCASE, OsRandom, PasswordError, RandomSource, SYMBOLS, UPPERCASE, entropy_bits,
    generate_password,
};

/// Yields `start, start + 1, …` modulo 256, after an optional prefix.
struct Counter {
    prefix: Vec<u8>,
    next: u8,
    consumed: usize,
}

impl Counter {
    fn new(start: u8) -> Self {
        Self::with_prefix(&[], start)
    }

    fn with_prefix(prefix: &[u8], start: u8) -> Self {
        Self {
            prefix: prefix.iter().rev().copied().collect(),
            next: start,
            consumed: 0,
        }
    }
}

impl RandomSource for Counter {
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), PasswordError> {
        for byte in bytes {
            *byte = match self.prefix.pop() {
                Some(value) => value,
                None => {
                    let value = self.next;
                    self.next = self.next.wrapping_add(1);
                    value
                }
            };
            self.consumed += 1;
        }
        Ok(())
    }
}

/// SplitMix64, a small deterministic generator good enough for distribution checks.
struct SplitMix(u64);

impl RandomSource for SplitMix {
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), PasswordError> {
        for chunk in bytes.chunks_mut(8) {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
        }
        Ok(())
    }
}

struct Constant(u8);

impl RandomSource for Constant {
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), PasswordError> {
        bytes.fill(self.0);
        Ok(())
    }
}

struct Failing;

impl RandomSource for Failing {
    fn fill(&mut self, _bytes: &mut [u8]) -> Result<(), PasswordError> {
        Err(PasswordError::RandomUnavailable)
    }
}

fn options(length: usize) -> GeneratorOptions {
    GeneratorOptions {
        length,
        ..GeneratorOptions::default()
    }
}

fn generate(options: &GeneratorOptions, random: &mut impl RandomSource) -> String {
    generate_password(options, random)
        .expect("generation succeeds")
        .expose()
        .to_owned()
}

#[test]
fn character_sets_are_as_documented() {
    assert_eq!(LOWERCASE, "abcdefghijklmnopqrstuvwxyz");
    assert_eq!(UPPERCASE, "ABCDEFGHIJKLMNOPQRSTUVWXYZ");
    assert_eq!(DIGITS, "0123456789");
    assert_eq!(SYMBOLS, "!#$%&*+-.:=?@^_~");
    assert_eq!(AMBIGUOUS_CHARACTERS, "0Oo1Il|");
    assert_eq!((GENERATOR_MIN_LENGTH, GENERATOR_MAX_LENGTH), (8, 128));
}

#[test]
fn defaults_are_twenty_characters_of_every_class_without_ambiguous_ones() {
    let defaults = GeneratorOptions::default();
    assert_eq!(defaults.length, 20);
    assert!(defaults.lowercase && defaults.uppercase && defaults.digits && defaults.symbols);
    assert!(defaults.avoid_ambiguous);
}

// Expected values derived independently with a Python version of the documented algorithm.
#[test]
fn a_fixed_byte_stream_gives_an_exact_password() {
    assert_eq!(generate(&options(8), &mut Counter::new(0)), "g%fhBe4a");
    assert_eq!(
        generate(&options(20), &mut Counter::new(100)),
        "F-VYL8PWXTRNQZKSJUMe"
    );
    let letters_and_digits = GeneratorOptions {
        length: 12,
        lowercase: true,
        uppercase: false,
        digits: true,
        symbols: false,
        avoid_ambiguous: false,
    };
    assert_eq!(
        generate(&letters_and_digits, &mut Counter::new(7)),
        "hjpnmrksl8qo"
    );
}

#[test]
fn bytes_that_would_bias_a_draw_are_rejected() {
    // The first draw picks one of 24 lower-case letters: bytes 240..=255 would favour the first
    // 16 letters, so they are skipped and the stream continues as if they were not there.
    let plain = generate(&options(8), &mut Counter::new(0));
    let mut with_rejections = Counter::with_prefix(&[240, 255, 250], 0);
    assert_eq!(generate(&options(8), &mut with_rejections), plain);
    let mut without = Counter::new(0);
    generate(&options(8), &mut without);
    assert_eq!(with_rejections.consumed, without.consumed + 3);
}

#[test]
fn a_source_that_only_gives_rejected_bytes_is_an_error_not_a_hang() {
    let lower_only = GeneratorOptions {
        length: 8,
        lowercase: true,
        uppercase: false,
        digits: false,
        symbols: false,
        avoid_ambiguous: false,
    };
    assert_eq!(
        generate_password(&lower_only, &mut Constant(255)).map(|_| ()),
        Err(PasswordError::RandomUnavailable)
    );
}

#[test]
fn a_failing_source_is_reported() {
    assert_eq!(
        generate_password(&options(16), &mut Failing).map(|_| ()),
        Err(PasswordError::RandomUnavailable)
    );
}

#[test]
fn length_limits_and_no_class_are_errors() {
    for length in [0, 1, GENERATOR_MIN_LENGTH - 1, GENERATOR_MAX_LENGTH + 1] {
        assert!(matches!(
            generate_password(&options(length), &mut SplitMix(1)),
            Err(PasswordError::InvalidInput(_))
        ));
    }
    for length in [GENERATOR_MIN_LENGTH, GENERATOR_MAX_LENGTH] {
        assert_eq!(generate(&options(length), &mut SplitMix(1)).len(), length);
    }
    let none = GeneratorOptions {
        lowercase: false,
        uppercase: false,
        digits: false,
        symbols: false,
        ..GeneratorOptions::default()
    };
    assert!(matches!(
        generate_password(&none, &mut SplitMix(1)),
        Err(PasswordError::InvalidInput(_))
    ));
}

#[test]
fn every_enabled_class_appears_and_no_other_character_does() {
    let mut combinations = Vec::new();
    for mask in 1u8..16 {
        for avoid_ambiguous in [false, true] {
            combinations.push(GeneratorOptions {
                length: GENERATOR_MIN_LENGTH,
                lowercase: mask & 1 != 0,
                uppercase: mask & 2 != 0,
                digits: mask & 4 != 0,
                symbols: mask & 8 != 0,
                avoid_ambiguous,
            });
        }
    }
    for options in &combinations {
        for seed in 0..300 {
            let password = generate(options, &mut SplitMix(seed));
            assert_eq!(password.len(), options.length);
            for (enabled, class) in [
                (options.lowercase, LOWERCASE),
                (options.uppercase, UPPERCASE),
                (options.digits, DIGITS),
                (options.symbols, SYMBOLS),
            ] {
                let present = password.chars().any(|c| class.contains(c));
                assert_eq!(present, enabled, "class {class} in a seed-{seed} password");
            }
            if options.avoid_ambiguous {
                assert!(!password.chars().any(|c| AMBIGUOUS_CHARACTERS.contains(c)));
            }
        }
    }
}

#[test]
fn characters_are_spread_as_the_algorithm_predicts() {
    // Each password has one forced character per class, drawn from that class, and the rest
    // drawn from the union. The expected count of a character is therefore
    // samples * (1/|class| + (length - classes)/|union|).
    let options = GeneratorOptions::default();
    let samples = 20_000usize;
    let mut random = SplitMix(0x5eed);
    let mut counts: HashMap<char, usize> = HashMap::new();
    for _ in 0..samples {
        for character in generate(&options, &mut random).chars() {
            *counts.entry(character).or_default() += 1;
        }
    }
    let classes: Vec<String> = [LOWERCASE, UPPERCASE, DIGITS, SYMBOLS]
        .iter()
        .map(|class| {
            class
                .chars()
                .filter(|c| !AMBIGUOUS_CHARACTERS.contains(*c))
                .collect()
        })
        .collect();
    let union: usize = classes.iter().map(String::len).sum();
    assert_eq!(union, 72);
    assert_eq!(counts.len(), union, "every allowed character occurs");
    let free = (options.length - classes.len()) as f64;
    let mut chi_squared = 0.0;
    for class in &classes {
        let expected = samples as f64 * (1.0 / class.len() as f64 + free / union as f64);
        for character in class.chars() {
            let observed = counts[&character] as f64;
            chi_squared += (observed - expected).powi(2) / expected;
        }
    }
    // 71 degrees of freedom: mean 71, standard deviation about 11.9.
    println!("character chi-squared: {chi_squared:.1}");
    assert!(chi_squared < 150.0, "chi-squared {chi_squared}");
}

#[test]
fn forced_characters_land_in_every_position_equally() {
    // Without an unbiased shuffle the forced digit would always sit in the same place.
    let options = GeneratorOptions {
        length: 8,
        lowercase: true,
        uppercase: false,
        digits: true,
        symbols: false,
        avoid_ambiguous: false,
    };
    let samples = 40_000usize;
    let mut random = SplitMix(42);
    let mut digits_at = [0usize; 8];
    for _ in 0..samples {
        for (position, character) in generate(&options, &mut random).chars().enumerate() {
            if character.is_ascii_digit() {
                digits_at[position] += 1;
            }
        }
    }
    let total: usize = digits_at.iter().sum();
    let expected = total as f64 / 8.0;
    let chi_squared: f64 = digits_at
        .iter()
        .map(|&observed| (observed as f64 - expected).powi(2) / expected)
        .sum();
    // 7 degrees of freedom: the 99.9th percentile is 24.3.
    println!("position chi-squared: {chi_squared:.1}");
    assert!(
        chi_squared < 24.3,
        "chi-squared {chi_squared}, counts {digits_at:?}"
    );
}

#[test]
fn os_random_gives_different_values_each_call() {
    let mut random = OsRandom;
    let mut first = [0u8; 32];
    let mut second = [0u8; 32];
    random.fill(&mut first).expect("the OS generator works");
    random.fill(&mut second).expect("the OS generator works");
    assert_ne!(first, second);
    let one = generate(&GeneratorOptions::default(), &mut OsRandom);
    let two = generate(&GeneratorOptions::default(), &mut OsRandom);
    assert_ne!(one, two);
    assert_eq!(one.len(), 20);
}

#[test]
fn entropy_matches_a_hand_calculation() {
    // Defaults: 24 lower, 24 upper, 8 digits, 16 symbols, 72 in all; 4 forced characters and 16
    // free ones: log2(24) + log2(24) + log2(8) + log2(16) + 16 * log2(72) = 114.889 bits.
    let expected = 2.0 * 24f64.log2() + 3.0 + 4.0 + 16.0 * 72f64.log2();
    assert!((entropy_bits(&GeneratorOptions::default()) - expected).abs() < 1e-9);
    assert!((entropy_bits(&GeneratorOptions::default()) - 114.889).abs() < 0.001);

    let digits_only = GeneratorOptions {
        length: 10,
        lowercase: false,
        uppercase: false,
        digits: true,
        symbols: false,
        avoid_ambiguous: false,
    };
    // One class: every character is a uniform draw from 10 digits.
    assert!((entropy_bits(&digits_only) - 10.0 * 10f64.log2()).abs() < 1e-9);

    let invalid = GeneratorOptions {
        length: 4,
        ..GeneratorOptions::default()
    };
    assert_eq!(entropy_bits(&invalid), 0.0);
}

#[test]
fn generated_passwords_are_accepted_by_the_store() {
    use mercury_passwords::{LoginKey, MemoryBackend, Origin, PasswordStore};
    let store = PasswordStore::new(MemoryBackend::new());
    let password =
        generate_password(&GeneratorOptions::default(), &mut SplitMix(9)).expect("generated");
    let key = LoginKey::form(
        Origin::parse("https://example.com").expect("valid"),
        "alice",
    );
    store
        .save(&key, &password)
        .expect("a generated password is valid");
}
