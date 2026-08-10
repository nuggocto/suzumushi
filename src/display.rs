// SPDX-License-Identifier: Apache-2.0

//! Bounded terminal-safe display projection for untrusted text.

/// Projects bytes into bounded UTF-8 with reserved escapes for controls and
/// invalid input.
#[must_use]
pub fn terminal_safe(input: &[u8], max_bytes: usize) -> String {
    let mut output = String::with_capacity(input.len().min(max_bytes));
    let mut remaining = input;
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                append_valid(&mut output, valid, max_bytes);
                break;
            }
            Err(error) => {
                let valid_bytes = error.valid_up_to();
                let Ok(valid) = std::str::from_utf8(&remaining[..valid_bytes]) else {
                    break;
                };
                if !append_valid(&mut output, valid, max_bytes) {
                    break;
                }
                let invalid_bytes = error
                    .error_len()
                    .unwrap_or_else(|| remaining.len() - valid_bytes);
                let invalid = &remaining[valid_bytes..valid_bytes + invalid_bytes];
                if invalid
                    .iter()
                    .any(|byte| !push_byte_escape(&mut output, *byte, max_bytes))
                {
                    break;
                }
                remaining = &remaining[valid_bytes + invalid_bytes..];
            }
        }
    }
    output
}

/// Copies already-safe UTF-8 without splitting a character or exceeding the byte bound.
#[must_use]
pub(crate) fn bounded_text(input: &str, max_bytes: usize) -> String {
    let mut output = String::with_capacity(input.len().min(max_bytes));
    for character in input.chars() {
        if output.len().saturating_add(character.len_utf8()) > max_bytes {
            break;
        }
        output.push(character);
    }
    output
}

fn append_valid(output: &mut String, valid: &str, max_bytes: usize) -> bool {
    for character in valid.chars() {
        if character == '\\' {
            if output.len().saturating_add(2) > max_bytes {
                return false;
            }
            output.push_str("\\\\");
        } else if character.is_control() {
            let required = character.len_utf8().saturating_mul(4);
            if output.len().saturating_add(required) > max_bytes {
                return false;
            }
            let mut bytes = [0; 4];
            for byte in character.encode_utf8(&mut bytes).as_bytes() {
                push_byte_escape_unchecked(output, *byte);
            }
        } else {
            if output.len().saturating_add(character.len_utf8()) > max_bytes {
                return false;
            }
            output.push(character);
        }
    }
    true
}

fn push_byte_escape(output: &mut String, byte: u8, max_bytes: usize) -> bool {
    if output.len().saturating_add(4) > max_bytes {
        return false;
    }
    push_byte_escape_unchecked(output, byte);
    true
}

fn push_byte_escape_unchecked(output: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    output.push('\\');
    output.push('x');
    output.push(char::from(HEX[usize::from(byte >> 4)]));
    output.push(char::from(HEX[usize::from(byte & 0x0f)]));
}

#[cfg(test)]
mod tests {
    use super::bounded_text;

    #[test]
    fn bounded_text_preserves_existing_escapes_and_utf8_boundaries() {
        assert_eq!(bounded_text(r"a\\b", 4), r"a\\b");
        assert_eq!(bounded_text("cricket", 4), "cric");
        assert_eq!(bounded_text("éé", 3), "é");
    }
}
