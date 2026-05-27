#[inline]
pub(crate) fn strip_sse_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    line.strip_prefix(&format!("{field}: "))
        .or_else(|| line.strip_prefix(&format!("{field}:")))
}

#[inline]
pub(crate) fn take_sse_block(buffer: &mut String) -> Option<String> {
    let mut best: Option<(usize, usize)> = None;

    for (delimiter, len) in [("\r\n\r\n", 4usize), ("\n\n", 2usize)] {
        if let Some(pos) = buffer.find(delimiter) {
            if match best {
                Some((best_pos, _)) => pos < best_pos,
                None => true,
            } {
                best = Some((pos, len));
            }
        }
    }

    let (pos, len) = best?;
    let block = buffer[..pos].to_string();
    buffer.drain(..pos + len);
    Some(block)
}

pub(crate) fn append_utf8_safe(buffer: &mut String, remainder: &mut Vec<u8>, new_bytes: &[u8]) {
    let (owned, bytes): (Option<Vec<u8>>, &[u8]) = if remainder.is_empty() {
        (None, new_bytes)
    } else if remainder.len() > 3 {
        buffer.push_str(&String::from_utf8_lossy(remainder));
        remainder.clear();
        (None, new_bytes)
    } else {
        let mut combined = std::mem::take(remainder);
        combined.extend_from_slice(new_bytes);
        (Some(combined), &[])
    };
    let input = owned.as_deref().unwrap_or(bytes);

    let mut pos = 0;
    loop {
        match std::str::from_utf8(&input[pos..]) {
            Ok(valid) => {
                buffer.push_str(valid);
                return;
            }
            Err(error) => {
                let valid_up_to = pos + error.valid_up_to();
                let valid_slice = &input[pos..valid_up_to];
                match std::str::from_utf8(valid_slice) {
                    Ok(valid) => buffer.push_str(valid),
                    Err(_) => buffer.push_str(&String::from_utf8_lossy(valid_slice)),
                }

                if let Some(invalid_len) = error.error_len() {
                    buffer.push('\u{FFFD}');
                    pos = valid_up_to + invalid_len;
                } else {
                    *remainder = input[valid_up_to..].to_vec();
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_sse_field_accepts_optional_space() {
        assert_eq!(
            strip_sse_field("data: {\"ok\":true}", "data"),
            Some("{\"ok\":true}")
        );
        assert_eq!(
            strip_sse_field("data:{\"ok\":true}", "data"),
            Some("{\"ok\":true}")
        );
        assert_eq!(strip_sse_field("event: done", "event"), Some("done"));
        assert_eq!(strip_sse_field("id:1", "data"), None);
    }

    #[test]
    fn take_sse_block_supports_lf_and_crlf() {
        let mut lf = "data: {\"ok\":true}\n\nrest".to_string();
        assert_eq!(
            take_sse_block(&mut lf),
            Some("data: {\"ok\":true}".to_string())
        );
        assert_eq!(lf, "rest");

        let mut crlf = "data: {\"ok\":true}\r\n\r\nrest".to_string();
        assert_eq!(
            take_sse_block(&mut crlf),
            Some("data: {\"ok\":true}".to_string())
        );
        assert_eq!(crlf, "rest");
    }

    #[test]
    fn append_utf8_safe_preserves_split_multibyte_characters() {
        let bytes = "你好".as_bytes();
        let mut buffer = String::new();
        let mut remainder = Vec::new();

        append_utf8_safe(&mut buffer, &mut remainder, &bytes[..2]);
        assert_eq!(buffer, "");
        assert_eq!(remainder, bytes[..2]);

        append_utf8_safe(&mut buffer, &mut remainder, &bytes[2..]);
        assert_eq!(buffer, "你好");
        assert!(remainder.is_empty());
    }
}
