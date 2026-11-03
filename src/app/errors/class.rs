//! The semantic classes errors fall into, and the matcher that reads a class
//! out of an error's name.

/// Declares the semantic classes in one place: the enum, its vocabulary, its
/// advice, the iteration list, and the name parse used by the Anchor table.
///
/// A hand-written `ALL` can fall out of step with the enum in a way exhaustive
/// `match` cannot catch, and the TSV's class keyword would be a third place to
/// typo. Generating all of them from one declaration removes both risks.
macro_rules! classes {
    ($($variant:ident [$($token:literal),+] $help:literal;)+) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
        pub enum Class { $($variant),+ }

        impl Class {
            pub const ALL: &'static [Class] = &[$(Class::$variant),+];

            /// The vocabulary that identifies this class in an error name.
            pub fn tokens(self) -> &'static [&'static str] {
                match self { $(Class::$variant => &[$($token),+]),+ }
            }

            /// What to do about an error of this class.
            pub fn help(self) -> &'static str {
                match self { $(Class::$variant => $help),+ }
            }

            pub fn from_name(name: &str) -> Option<Class> {
                match name { $(stringify!($variant) => Some(Class::$variant),)+ _ => None }
            }
        }
    };
}

classes! {
    Address ["address"] "the account address does not match the one the program requires";
    Already ["already", "exists", "duplicate"] "it already exists — change the seed, or use init_if_needed";
    Ata ["associated", "ata"] "this is not the associated token account for that owner and mint";
    Decimals ["decimals"] "the decimals passed do not match the mint";
    Expired ["expired", "stale", "deadline", "timeout"] "past its validity window — refresh it and resend";
    Instruction ["instruction", "discriminator", "fallback"] "the deployed program does not recognise this instruction — redeploy, or regenerate the client from the deployed IDL";
    Insufficient ["insufficient", "funds", "liquidity"] "the source does not hold enough — check the amount against the live balance";
    Mint ["mint"] "the token account's mint does not match the one the instruction expects";
    NotSigner ["signer", "sign", "signature"] "add the missing signer to the transaction";
    NotWritable ["writable", "mutable", "readonly"] "the account must be passed as writable";
    Overflow ["overflow", "underflow"] "the arithmetic wrapped — check the amount and the decimals";
    Paused ["paused", "halted", "disabled", "frozen"] "administratively disabled — nothing to fix client-side";
    Rent ["rent", "exempt"] "the account is below the rent-exempt minimum — fund it first";
    Seeds ["seed", "seeds", "pda", "bump", "derived"] "re-derive the PDA: same seeds, same bump, same program id";
    Slippage ["slippage", "price", "quote"] "the quote moved before the transaction landed — widen the tolerance or requote";
    Unauthorized ["unauthorized", "authority", "admin", "permission"] "the signer is not the authority the program stores for this account";
    Uninitialized ["uninitialized", "init"] "the account does not exist yet — run the init instruction first";
    WrongOwner ["owner", "ownership"] "check which program owns the account you passed";
}

/// Shortest token overlap we treat as a prefix match. Below this, fragments
/// like `mut` collide with unrelated words.
const MIN_PREFIX: usize = 4;

/// Splits an error name into its distinct lowercase words.
///
/// Handles `camelCase`, `snake_case`, digit runs, and acronyms, so
/// `InvalidPDAOwner` yields `invalid`, `pda`, `owner`. Sorted and deduplicated:
/// each distinct word must score once, or a vocabulary with overlapping entries
/// would count one word twice.
fn tokenize(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_alphanumeric() {
            i += 1;
            continue;
        }
        let end = word_end(&chars, i);
        tokens.push(chars[i..end].iter().collect::<String>().to_lowercase());
        i = end;
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

/// How far a run of characters matching `is_part` extends from `from`.
fn run(chars: &[char], from: usize, is_part: impl Fn(char) -> bool) -> usize {
    from + chars[from..].iter().take_while(|c| is_part(**c)).count()
}

/// Where the word starting at `start` ends: a digit run, a lowercase run, a
/// capitalised word, or an acronym run.
fn word_end(chars: &[char], start: usize) -> usize {
    if chars[start].is_ascii_digit() {
        return run(chars, start, |c| c.is_ascii_digit());
    }
    if !chars[start].is_uppercase() {
        // From start + 1: this character is already known to belong to the
        // word, and starting the run past it guarantees the scan advances even
        // for uncased alphanumerics like 中, which are neither upper nor lower.
        return run(chars, start + 1, char::is_lowercase);
    }
    if chars.get(start + 1).is_some_and(|c| c.is_lowercase()) {
        return run(chars, start + 1, char::is_lowercase);
    }
    acronym_end(chars, start + 1)
}

/// Where an acronym run ends: the last capital before a capitalised word, since
/// that capital starts the next word rather than ending this one.
fn acronym_end(chars: &[char], from: usize) -> usize {
    let mut i = from;
    while chars.get(i).is_some_and(|c| c.is_uppercase()) && !chars.get(i + 1).is_some_and(|c| c.is_lowercase()) {
        i += 1;
    }
    i
}

/// Scores a class against a name's words: 2 for an exact match, 1 for a common
/// prefix. Best match per word, so overlapping vocabulary entries such as
/// `init` and `uninitialized` cannot both count for one word.
fn score(tokens: &[String], class: Class) -> u32 {
    tokens
        .iter()
        .map(|token| {
            class
                .tokens()
                .iter()
                .map(|entry| {
                    if token == entry {
                        2
                    } else if token.len() >= MIN_PREFIX
                        && entry.len() >= MIN_PREFIX
                        && (token.starts_with(entry) || entry.starts_with(token.as_str()))
                    {
                        1
                    } else {
                        0
                    }
                })
                .max()
                .unwrap_or(0)
        })
        .sum()
}

/// The class whose vocabulary best fits an error name.
///
/// None when nothing matches, and also when two classes tie: a tie means the
/// name is ambiguous, and staying silent beats picking arbitrarily between two
/// different fixes.
pub fn classify_name(name: &str) -> Option<Class> {
    let tokens = tokenize(name);
    let mut best: Option<(u32, Class)> = None;
    let mut tied = false;
    for &class in Class::ALL {
        let scored = score(&tokens, class);
        if scored == 0 {
            continue;
        }
        match best {
            Some((top, _)) if scored > top => {
                best = Some((scored, class));
                tied = false;
            }
            Some((top, _)) if scored == top => tied = true,
            Some(_) => {}
            None => best = Some((scored, class)),
        }
    }
    match best {
        Some((_, class)) if !tied => Some(class),
        _ => None,
    }
}
