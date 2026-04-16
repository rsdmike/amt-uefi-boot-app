/// Length of a null-terminated ASCII byte string.
pub fn ascii_len(s: &[u8]) -> usize {
    s.iter().position(|&b| b == 0).unwrap_or(s.len())
}

/// Find `needle` in `haystack` (both null-terminated or bounded by slice length).
pub fn ascii_find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let h_len = ascii_len(haystack);
    let n_len = ascii_len(needle);
    if n_len == 0 {
        return Some(0);
    }
    if n_len > h_len {
        return None;
    }
    for i in 0..=(h_len - n_len) {
        if haystack[i..i + n_len] == needle[..n_len] {
            return Some(i);
        }
    }
    None
}

/// Append bytes from `s` into `buf` at `pos`. Returns new position.
pub fn ascii_append(buf: &mut [u8], pos: usize, s: &[u8]) -> usize {
    let s_len = ascii_len(s);
    let copy_len = s_len.min(buf.len().saturating_sub(pos));
    if copy_len > 0 {
        buf[pos..pos + copy_len].copy_from_slice(&s[..copy_len]);
    }
    pos + copy_len
}

/// Append a raw byte slice (not null-terminated) into `buf` at `pos`. Returns new position.
pub fn append_bytes(buf: &mut [u8], pos: usize, s: &[u8]) -> usize {
    let copy_len = s.len().min(buf.len().saturating_sub(pos));
    if copy_len > 0 {
        buf[pos..pos + copy_len].copy_from_slice(&s[..copy_len]);
    }
    pos + copy_len
}

/// Append a u32 as decimal ASCII into `buf` at `pos`. Returns new position.
pub fn append_u32(buf: &mut [u8], pos: usize, val: u32) -> usize {
    if val == 0 {
        return append_bytes(buf, pos, b"0");
    }
    let mut tmp = [0u8; 10];
    let mut n = val;
    let mut i = 0;
    while n > 0 {
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    // Reverse into buf
    let mut p = pos;
    while i > 0 {
        i -= 1;
        if p < buf.len() {
            buf[p] = tmp[i];
            p += 1;
        }
    }
    p
}

/// Append a u32 as 8-digit zero-padded hex into `buf` at `pos`. Returns new position.
pub fn append_hex32(buf: &mut [u8], pos: usize, val: u32) -> usize {
    let hex = b"0123456789abcdef";
    let mut p = pos;
    for i in (0..8).rev() {
        if p < buf.len() {
            buf[p] = hex[((val >> (i * 4)) & 0xF) as usize];
            p += 1;
        }
    }
    p
}

/// Case-insensitive ASCII byte comparison.
pub fn ascii_eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        let ca = if a[i] >= b'A' && a[i] <= b'Z' { a[i] + 32 } else { a[i] };
        let cb = if b[i] >= b'A' && b[i] <= b'Z' { b[i] + 32 } else { b[i] };
        if ca != cb {
            return false;
        }
    }
    true
}

/// Extract a field value from a string like `key="value"` or `key=value`.
/// Returns the value as a byte slice copied into `out`, and the length.
pub fn extract_field(src: &[u8], key: &[u8], out: &mut [u8]) -> usize {
    let src_len = ascii_len(src);
    let key_len = ascii_len(key);
    if key_len == 0 || src_len == 0 {
        return 0;
    }

    // Search for key= in src
    let mut i = 0;
    while i + key_len + 1 <= src_len {
        if ascii_eq_ignore_case(&src[i..i + key_len], &key[..key_len]) && src[i + key_len] == b'=' {
            let val_start = i + key_len + 1;
            // Check if quoted
            if val_start < src_len && src[val_start] == b'"' {
                // Find closing quote
                let inner_start = val_start + 1;
                let mut j = inner_start;
                while j < src_len && src[j] != b'"' {
                    j += 1;
                }
                let len = j - inner_start;
                let copy = len.min(out.len());
                out[..copy].copy_from_slice(&src[inner_start..inner_start + copy]);
                if copy < out.len() {
                    out[copy] = 0;
                }
                return copy;
            } else {
                // Unquoted: read until comma, space, or end
                let mut j = val_start;
                while j < src_len && src[j] != b',' && src[j] != b' ' && src[j] != b'\r' && src[j] != b'\n' {
                    j += 1;
                }
                let len = j - val_start;
                let copy = len.min(out.len());
                out[..copy].copy_from_slice(&src[val_start..val_start + copy]);
                if copy < out.len() {
                    out[copy] = 0;
                }
                return copy;
            }
        }
        i += 1;
    }
    0
}
