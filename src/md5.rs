/// MD5 (RFC 1321) - pure no_std implementation.

struct Md5Ctx {
    state: [u32; 4],
    count: u64,
    buffer: [u8; 64],
}

#[inline(always)]
fn f(x: u32, y: u32, z: u32) -> u32 { (x & y) | ((!x) & z) }
#[inline(always)]
fn g(x: u32, y: u32, z: u32) -> u32 { (x & z) | (y & (!z)) }
#[inline(always)]
fn h(x: u32, y: u32, z: u32) -> u32 { x ^ y ^ z }
#[inline(always)]
fn ii(x: u32, y: u32, z: u32) -> u32 { y ^ (x | (!z)) }

macro_rules! step {
    ($func:ident, $a:expr, $b:expr, $c:expr, $d:expr, $x:expr, $t:expr, $s:expr) => {
        $a = $a.wrapping_add($func($b, $c, $d)).wrapping_add($x).wrapping_add($t);
        $a = $a.rotate_left($s);
        $a = $a.wrapping_add($b);
    };
}

fn md5_transform(state: &mut [u32; 4], block: &[u8]) {
    let (mut a, mut b, mut c, mut d) = (state[0], state[1], state[2], state[3]);

    // Decode block into 16 little-endian 32-bit words
    let mut m = [0u32; 16];
    for i in 0..16 {
        m[i] = u32::from_le_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }

    // Round 1
    step!(f, a, b, c, d, m[ 0], 0xd76aa478,  7);
    step!(f, d, a, b, c, m[ 1], 0xe8c7b756, 12);
    step!(f, c, d, a, b, m[ 2], 0x242070db, 17);
    step!(f, b, c, d, a, m[ 3], 0xc1bdceee, 22);
    step!(f, a, b, c, d, m[ 4], 0xf57c0faf,  7);
    step!(f, d, a, b, c, m[ 5], 0x4787c62a, 12);
    step!(f, c, d, a, b, m[ 6], 0xa8304613, 17);
    step!(f, b, c, d, a, m[ 7], 0xfd469501, 22);
    step!(f, a, b, c, d, m[ 8], 0x698098d8,  7);
    step!(f, d, a, b, c, m[ 9], 0x8b44f7af, 12);
    step!(f, c, d, a, b, m[10], 0xffff5bb1, 17);
    step!(f, b, c, d, a, m[11], 0x895cd7be, 22);
    step!(f, a, b, c, d, m[12], 0x6b901122,  7);
    step!(f, d, a, b, c, m[13], 0xfd987193, 12);
    step!(f, c, d, a, b, m[14], 0xa679438e, 17);
    step!(f, b, c, d, a, m[15], 0x49b40821, 22);

    // Round 2
    step!(g, a, b, c, d, m[ 1], 0xf61e2562,  5);
    step!(g, d, a, b, c, m[ 6], 0xc040b340,  9);
    step!(g, c, d, a, b, m[11], 0x265e5a51, 14);
    step!(g, b, c, d, a, m[ 0], 0xe9b6c7aa, 20);
    step!(g, a, b, c, d, m[ 5], 0xd62f105d,  5);
    step!(g, d, a, b, c, m[10], 0x02441453,  9);
    step!(g, c, d, a, b, m[15], 0xd8a1e681, 14);
    step!(g, b, c, d, a, m[ 4], 0xe7d3fbc8, 20);
    step!(g, a, b, c, d, m[ 9], 0x21e1cde6,  5);
    step!(g, d, a, b, c, m[14], 0xc33707d6,  9);
    step!(g, c, d, a, b, m[ 3], 0xf4d50d87, 14);
    step!(g, b, c, d, a, m[ 8], 0x455a14ed, 20);
    step!(g, a, b, c, d, m[13], 0xa9e3e905,  5);
    step!(g, d, a, b, c, m[ 2], 0xfcefa3f8,  9);
    step!(g, c, d, a, b, m[ 7], 0x676f02d9, 14);
    step!(g, b, c, d, a, m[12], 0x8d2a4c8a, 20);

    // Round 3
    step!(h, a, b, c, d, m[ 5], 0xfffa3942,  4);
    step!(h, d, a, b, c, m[ 8], 0x8771f681, 11);
    step!(h, c, d, a, b, m[11], 0x6d9d6122, 16);
    step!(h, b, c, d, a, m[14], 0xfde5380c, 23);
    step!(h, a, b, c, d, m[ 1], 0xa4beea44,  4);
    step!(h, d, a, b, c, m[ 4], 0x4bdecfa9, 11);
    step!(h, c, d, a, b, m[ 7], 0xf6bb4b60, 16);
    step!(h, b, c, d, a, m[10], 0xbebfbc70, 23);
    step!(h, a, b, c, d, m[13], 0x289b7ec6,  4);
    step!(h, d, a, b, c, m[ 0], 0xeaa127fa, 11);
    step!(h, c, d, a, b, m[ 3], 0xd4ef3085, 16);
    step!(h, b, c, d, a, m[ 6], 0x04881d05, 23);
    step!(h, a, b, c, d, m[ 9], 0xd9d4d039,  4);
    step!(h, d, a, b, c, m[12], 0xe6db99e5, 11);
    step!(h, c, d, a, b, m[15], 0x1fa27cf8, 16);
    step!(h, b, c, d, a, m[ 2], 0xc4ac5665, 23);

    // Round 4
    step!(ii, a, b, c, d, m[ 0], 0xf4292244,  6);
    step!(ii, d, a, b, c, m[ 7], 0x432aff97, 10);
    step!(ii, c, d, a, b, m[14], 0xab9423a7, 15);
    step!(ii, b, c, d, a, m[ 5], 0xfc93a039, 21);
    step!(ii, a, b, c, d, m[12], 0x655b59c3,  6);
    step!(ii, d, a, b, c, m[ 3], 0x8f0ccc92, 10);
    step!(ii, c, d, a, b, m[10], 0xffeff47d, 15);
    step!(ii, b, c, d, a, m[ 1], 0x85845dd1, 21);
    step!(ii, a, b, c, d, m[ 8], 0x6fa87e4f,  6);
    step!(ii, d, a, b, c, m[15], 0xfe2ce6e0, 10);
    step!(ii, c, d, a, b, m[ 6], 0xa3014314, 15);
    step!(ii, b, c, d, a, m[13], 0x4e0811a1, 21);
    step!(ii, a, b, c, d, m[ 4], 0xf7537e82,  6);
    step!(ii, d, a, b, c, m[11], 0xbd3af235, 10);
    step!(ii, c, d, a, b, m[ 2], 0x2ad7d2bb, 15);
    step!(ii, b, c, d, a, m[ 9], 0xeb86d391, 21);

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

impl Md5Ctx {
    fn new() -> Self {
        Md5Ctx {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            count: 0,
            buffer: [0; 64],
        }
    }

    fn update(&mut self, data: &[u8]) {
        let index = ((self.count / 8) % 64) as usize;
        self.count += (data.len() as u64) * 8;

        let mut i = 0usize;

        // Fill existing partial block
        if index > 0 {
            let part_len = 64 - index;
            if data.len() >= part_len {
                self.buffer[index..index + part_len].copy_from_slice(&data[..part_len]);
                let block: [u8; 64] = {
                    let mut b = [0u8; 64];
                    b.copy_from_slice(&self.buffer);
                    b
                };
                md5_transform(&mut self.state, &block);
                i = part_len;
            } else {
                self.buffer[index..index + data.len()].copy_from_slice(data);
                return;
            }
        }

        // Process full blocks
        while i + 64 <= data.len() {
            md5_transform(&mut self.state, &data[i..i + 64]);
            i += 64;
        }

        // Buffer remaining
        let remaining = data.len() - i;
        if remaining > 0 {
            self.buffer[..remaining].copy_from_slice(&data[i..]);
        }
    }

    fn finalize(&mut self, digest: &mut [u8; 16]) {
        let original_count = self.count;

        // Pad with 0x80 followed by zeros
        let index = ((self.count / 8) % 64) as usize;
        let pad_len = if index < 56 { 56 - index } else { 120 - index };

        let mut padding = [0u8; 64];
        padding[0] = 0x80;
        self.update(&padding[..pad_len]);

        // Append original length in bits (little-endian 64-bit)
        let mut bits = [0u8; 8];
        for i in 0..8 {
            bits[i] = (original_count >> (i * 8)) as u8;
        }
        self.update(&bits);

        // Output state as little-endian bytes
        for i in 0..4 {
            digest[i * 4] = self.state[i] as u8;
            digest[i * 4 + 1] = (self.state[i] >> 8) as u8;
            digest[i * 4 + 2] = (self.state[i] >> 16) as u8;
            digest[i * 4 + 3] = (self.state[i] >> 24) as u8;
        }
    }
}

/// Compute MD5 hash of `data`, writing 16-byte digest to `digest`.
pub fn md5_hash(data: &[u8], digest: &mut [u8; 16]) {
    let mut ctx = Md5Ctx::new();
    ctx.update(data);
    ctx.finalize(digest);
}

/// Convert a 16-byte digest to a 32-character hex string (null-terminated, 33 bytes).
pub fn md5_hex(digest: &[u8; 16], hex: &mut [u8; 33]) {
    const HEXTAB: &[u8; 16] = b"0123456789abcdef";
    for i in 0..16 {
        hex[i * 2] = HEXTAB[(digest[i] >> 4) as usize];
        hex[i * 2 + 1] = HEXTAB[(digest[i] & 0x0F) as usize];
    }
    hex[32] = 0;
}
