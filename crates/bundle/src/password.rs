//! Bundle passwords. Anyone with a bundle and its password gets everything
//! in it (API keys included), so the password has to be strong.

/// Shortest password accepted.
const MIN_CHARS: usize = 12;
/// Least estimated strength accepted, in bits.
const MIN_BITS: f64 = 64.0;

/// Why a password isn't strong enough, or `None` if it is. A rough
/// estimate: the length, the kinds of characters used, and how much it
/// repeats.
pub fn password_problem(password: &str) -> Option<String> {
    let chars: Vec<char> = password.chars().collect();
    if chars.len() < MIN_CHARS {
        return Some(format!("Use at least {MIN_CHARS} characters."));
    }
    let mut pool = 0u32;
    let has = |f: fn(&char) -> bool| chars.iter().any(f);
    if has(|c| c.is_ascii_lowercase()) {
        pool += 26;
    }
    if has(|c| c.is_ascii_uppercase()) {
        pool += 26;
    }
    if has(|c| c.is_ascii_digit()) {
        pool += 10;
    }
    if has(|c| c.is_ascii_punctuation() || *c == ' ') {
        pool += 33;
    }
    if has(|c| !c.is_ascii()) {
        pool += 100;
    }
    let mut distinct: Vec<char> = chars.clone();
    distinct.sort_unstable();
    distinct.dedup();
    // Repeats count for little: "aaaaaaaaaaaaaaaa" isn't 16 characters' worth.
    let effective = distinct.len().max(chars.len() / 3).min(chars.len());
    let bits = effective as f64 * f64::from(pool.max(1)).log2();
    if bits < MIN_BITS || distinct.len() < 6 {
        return Some(
            "That password is too easy to guess. Make it longer and less predictable, or use a generated one."
                .into(),
        );
    }
    None
}

/// A new random password: five groups of five characters from a
/// 32-character alphabet without look-alikes (about 125 bits).
pub fn generate_password() -> Result<String, String> {
    const ALPHABET: &[u8; 32] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let bytes = crate::random::<25>()?;
    let mut out = String::with_capacity(29);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            out.push('-');
        }
        // 32 divides 256, so every character is equally likely.
        out.push(ALPHABET[(*b & 31) as usize] as char);
    }
    Ok(out)
}
