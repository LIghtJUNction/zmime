//! Small bounded scanning kernels. No overreads, including at page boundaries.

#[inline]
pub(crate) fn find_byte(bytes: &[u8], needle: u8) -> Option<usize> {
    #[cfg(all(feature = "simd", target_arch = "x86_64"))]
    if bytes.len() >= 128 && crate::cpu::avx2() {
        // SAFETY: the cached detector proves AVX2 and OS vector-state support.
        // The kernel independently bounds every access to the supplied slice.
        return unsafe { avx2_byte(bytes, needle) };
    }
    #[cfg(all(feature = "simd", target_arch = "x86_64", target_feature = "sse2"))]
    {
        return x86(bytes, needle);
    }
    #[cfg(all(feature = "simd", target_arch = "aarch64", target_feature = "neon"))]
    {
        return neon(bytes, needle);
    }
    #[allow(unreachable_code)]
    portable(bytes, needle)
}

#[inline]
pub(crate) fn portable(bytes: &[u8], needle: u8) -> Option<usize> {
    let repeated = u64::from_ne_bytes([needle; 8]);
    let mut i = 0;
    while bytes.len() - i >= 8 {
        let b = &bytes[i..i + 8];
        let x = u64::from_ne_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) ^ repeated;
        if x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080 != 0 {
            return b.iter().position(|&v| v == needle).map(|p| i + p);
        }
        i += 8;
    }
    bytes[i..].iter().position(|&v| v == needle).map(|p| i + p)
}

#[cfg(all(feature = "simd", target_arch = "x86_64", target_feature = "sse2"))]
#[inline]
fn x86(bytes: &[u8], needle: u8) -> Option<usize> {
    use core::arch::x86_64::{
        _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi8,
    };
    let mut i = 0;
    // SAFETY: this function is compiled only when SSE2 is guaranteed by the
    // target. Each unaligned load is guarded by >= 16 remaining bytes. The
    // pointer originates from this live shared slice; loads do not write.
    unsafe {
        let target = _mm_set1_epi8(needle as i8);
        while bytes.len() - i >= 64 {
            let a = _mm_cmpeq_epi8(_mm_loadu_si128(bytes.as_ptr().add(i).cast()), target);
            let b = _mm_cmpeq_epi8(_mm_loadu_si128(bytes.as_ptr().add(i + 16).cast()), target);
            let c = _mm_cmpeq_epi8(_mm_loadu_si128(bytes.as_ptr().add(i + 32).cast()), target);
            let d = _mm_cmpeq_epi8(_mm_loadu_si128(bytes.as_ptr().add(i + 48).cast()), target);
            if _mm_movemask_epi8(_mm_or_si128(_mm_or_si128(a, b), _mm_or_si128(c, d))) != 0 {
                for (lane, mask) in [
                    _mm_movemask_epi8(a),
                    _mm_movemask_epi8(b),
                    _mm_movemask_epi8(c),
                    _mm_movemask_epi8(d),
                ]
                .iter()
                .enumerate()
                {
                    if *mask != 0 {
                        return Some(i + lane * 16 + mask.trailing_zeros() as usize);
                    }
                }
            }
            i += 64;
        }
        while bytes.len() - i >= 16 {
            let block = _mm_loadu_si128(bytes.as_ptr().add(i).cast());
            let mask = _mm_movemask_epi8(_mm_cmpeq_epi8(block, target)) as u32;
            if mask != 0 {
                return Some(i + mask.trailing_zeros() as usize);
            }
            i += 16;
        }
    }
    portable(&bytes[i..], needle).map(|p| i + p)
}

#[cfg(all(feature = "simd", target_arch = "aarch64", target_feature = "neon"))]
#[inline]
fn neon(bytes: &[u8], needle: u8) -> Option<usize> {
    use core::arch::aarch64::{vceqq_u8, vdupq_n_u8, vld1q_u8, vmaxvq_u8, vorrq_u8};
    let mut i = 0;
    // SAFETY: NEON is guaranteed by cfg(target_feature="neon"). The loop
    // condition proves each 16-byte load lies entirely in the input slice.
    // All pointers are read-only and no alignment stronger than u8 is needed.
    unsafe {
        let target = vdupq_n_u8(needle);
        while bytes.len() - i >= 64 {
            let a = vceqq_u8(vld1q_u8(bytes.as_ptr().add(i)), target);
            let b = vceqq_u8(vld1q_u8(bytes.as_ptr().add(i + 16)), target);
            let c = vceqq_u8(vld1q_u8(bytes.as_ptr().add(i + 32)), target);
            let d = vceqq_u8(vld1q_u8(bytes.as_ptr().add(i + 48)), target);
            if vmaxvq_u8(vorrq_u8(vorrq_u8(a, b), vorrq_u8(c, d))) != 0 {
                return bytes[i..i + 64]
                    .iter()
                    .position(|&v| v == needle)
                    .map(|p| i + p);
            }
            i += 64;
        }
        while bytes.len() - i >= 16 {
            let matches = vceqq_u8(vld1q_u8(bytes.as_ptr().add(i)), target);
            if vmaxvq_u8(matches) != 0 {
                return bytes[i..i + 16]
                    .iter()
                    .position(|&v| v == needle)
                    .map(|p| i + p);
            }
            i += 16;
        }
    }
    portable(&bytes[i..], needle).map(|p| i + p)
}

/// Find a forbidden field value octet, admitting HTAB and bytes >= 128.
#[inline]
pub(crate) fn find_header_control(bytes: &[u8]) -> Option<usize> {
    #[cfg(all(feature = "simd", target_arch = "x86_64"))]
    if bytes.len() >= 128 && crate::cpu::avx2() {
        // SAFETY: runtime feature checks cover AVX2 and OS state. The kernel
        // reads complete blocks only and leaves the short tail to safe Rust.
        return unsafe { avx2_control(bytes) };
    }
    let mut i = 0;
    #[cfg(all(feature = "simd", target_arch = "x86_64", target_feature = "sse2"))]
    {
        use core::arch::x86_64::*;
        // SAFETY: SSE2 is a target guarantee, and every load is completely
        // inside the input slice. The kernel performs only read-only loads.
        unsafe {
            let bias = _mm_set1_epi8(-128);
            let bound = _mm_set1_epi8(-96);
            let tab = _mm_set1_epi8(9);
            let del = _mm_set1_epi8(127);
            while bytes.len() - i >= 16 {
                let block = _mm_loadu_si128(bytes.as_ptr().add(i).cast());
                let low = _mm_cmpgt_epi8(bound, _mm_xor_si128(block, bias));
                let invalid = _mm_or_si128(
                    _mm_andnot_si128(_mm_cmpeq_epi8(block, tab), low),
                    _mm_cmpeq_epi8(block, del),
                );
                let mask = _mm_movemask_epi8(invalid) as u32;
                if mask != 0 {
                    return Some(i + mask.trailing_zeros() as usize);
                }
                i += 16;
            }
        }
    }
    #[cfg(all(feature = "simd", target_arch = "aarch64", target_feature = "neon"))]
    {
        use core::arch::aarch64::*;
        // SAFETY: NEON is enabled for this target; each unaligned read covers
        // 16 existing bytes. All SIMD state is local and no memory is written.
        unsafe {
            while bytes.len() - i >= 16 {
                let block = vld1q_u8(bytes.as_ptr().add(i));
                let low = vcltq_u8(block, vdupq_n_u8(32));
                let invalid = vorrq_u8(
                    vbicq_u8(low, vceqq_u8(block, vdupq_n_u8(9))),
                    vceqq_u8(block, vdupq_n_u8(127)),
                );
                if vmaxvq_u8(invalid) != 0 {
                    return bytes[i..i + 16]
                        .iter()
                        .position(|&b| is_control(b))
                        .map(|p| i + p);
                }
                i += 16;
            }
        }
    }
    while bytes.len() - i >= 8 {
        let b = &bytes[i..i + 8];
        let x = u64::from_ne_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        let low = x.wrapping_sub(0x2020_2020_2020_2020) & !x;
        let y = x ^ 0x7f7f_7f7f_7f7f_7f7f;
        let del = y.wrapping_sub(0x0101_0101_0101_0101) & !y;
        if (low | del) & 0x8080_8080_8080_8080 != 0 {
            if let Some(n) = b.iter().position(|&v| is_control(v)) {
                return Some(i + n);
            }
        }
        i += 8;
    }
    bytes[i..]
        .iter()
        .position(|&b| is_control(b))
        .map(|p| i + p)
}
fn is_control(b: u8) -> bool {
    b < 32 && b != b'\t' || b == 127
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn avx2_byte(bytes: &[u8], needle: u8) -> Option<usize> {
    use core::arch::x86_64::*;
    let mut i = 0;
    // SAFETY: the safe dispatcher proves AVX2/OS support. Every load stays
    // within bytes because the loop tests remaining length before advancing.
    // Unaligned loads accept u8 alignment. There are no stores or aliases.
    unsafe {
        let target = _mm256_set1_epi8(needle as i8);
        while bytes.len() - i >= 128 {
            let a = _mm256_cmpeq_epi8(_mm256_loadu_si256(bytes.as_ptr().add(i).cast()), target);
            let b = _mm256_cmpeq_epi8(
                _mm256_loadu_si256(bytes.as_ptr().add(i + 32).cast()),
                target,
            );
            let c = _mm256_cmpeq_epi8(
                _mm256_loadu_si256(bytes.as_ptr().add(i + 64).cast()),
                target,
            );
            let d = _mm256_cmpeq_epi8(
                _mm256_loadu_si256(bytes.as_ptr().add(i + 96).cast()),
                target,
            );
            let any = _mm256_or_si256(_mm256_or_si256(a, b), _mm256_or_si256(c, d));
            if _mm256_movemask_epi8(any) != 0 {
                for (lane, mask) in [
                    _mm256_movemask_epi8(a),
                    _mm256_movemask_epi8(b),
                    _mm256_movemask_epi8(c),
                    _mm256_movemask_epi8(d),
                ]
                .iter()
                .enumerate()
                {
                    if *mask != 0 {
                        return Some(i + lane * 32 + mask.trailing_zeros() as usize);
                    }
                }
            }
            i += 128;
        }
        while bytes.len() - i >= 32 {
            let block = _mm256_loadu_si256(bytes.as_ptr().add(i).cast());
            let mask = _mm256_movemask_epi8(_mm256_cmpeq_epi8(block, target)) as u32;
            if mask != 0 {
                return Some(i + mask.trailing_zeros() as usize);
            }
            i += 32;
        }
    }
    portable(&bytes[i..], needle).map(|p| i + p)
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn avx2_control(bytes: &[u8]) -> Option<usize> {
    use core::arch::x86_64::*;
    let mut i = 0;
    // SAFETY: AVX2 and OS state are proven by the safe dispatcher. Each load
    // reads exactly 32 in-bounds bytes. Arithmetic is on local registers only.
    unsafe {
        let bias = _mm256_set1_epi8(-128);
        let bound = _mm256_set1_epi8(-96);
        let tab = _mm256_set1_epi8(9);
        let del = _mm256_set1_epi8(127);
        while bytes.len() - i >= 32 {
            let block = _mm256_loadu_si256(bytes.as_ptr().add(i).cast());
            let low = _mm256_cmpgt_epi8(bound, _mm256_xor_si256(block, bias));
            let invalid = _mm256_or_si256(
                _mm256_andnot_si256(_mm256_cmpeq_epi8(block, tab), low),
                _mm256_cmpeq_epi8(block, del),
            );
            let mask = _mm256_movemask_epi8(invalid) as u32;
            if mask != 0 {
                return Some(i + mask.trailing_zeros() as usize);
            }
            i += 32;
        }
    }
    bytes[i..]
        .iter()
        .position(|&b| is_control(b))
        .map(|p| i + p)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_alignment_length_and_byte_matches_reference() {
        let mut bytes = [0u8; 320];
        let mut seed = 7u64;
        for byte in &mut bytes {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *byte = seed as u8;
        }
        let rounds = if cfg!(miri) { 2 } else { 32 };
        for start in 0..rounds {
            for len in 0..=128 {
                for needle in [0, 1, 10, 13, 45, 58, 127, 128, 255] {
                    let s = &bytes[start..start + len];
                    let reference = s.iter().position(|&b| b == needle);
                    assert_eq!(portable(s, needle), reference);
                    assert_eq!(find_byte(s, needle), reference);
                }
            }
        }
    }
    #[test]
    fn header_controls_match_reference() {
        let mut bytes = [b'x'; 256];
        let max = if cfg!(miri) { 20 } else { 160 };
        for position in 0..max {
            for value in 0..=255 {
                if cfg!(miri) && !matches!(value, 0 | 8 | 9 | 10 | 31 | 32 | 126 | 127 | 128 | 255)
                {
                    continue;
                }
                bytes[position] = value;
                for start in 0..=position.min(7) {
                    let input = &bytes[start..];
                    assert_eq!(
                        find_header_control(input),
                        input.iter().position(|&b| is_control(b))
                    );
                }
                bytes[position] = b'x';
            }
        }
    }
    #[test]
    fn long_blocks_and_vector_tails_do_not_overread() {
        let mut data = [b'x'; 600];
        let lengths = [
            0, 1, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257, 511,
        ];
        for start in 0..if cfg!(miri) { 2 } else { 32 } {
            for len in lengths {
                assert_eq!(find_byte(&data[start..start + len], b'-'), None);
                assert_eq!(find_header_control(&data[start..start + len]), None);
                if len > 0 {
                    data[start + len - 1] = b'\n';
                    assert_eq!(find_byte(&data[start..start + len], b'\n'), Some(len - 1));
                    assert_eq!(
                        find_header_control(&data[start..start + len]),
                        Some(len - 1)
                    );
                    data[start + len - 1] = b'x';
                }
            }
        }
    }
}
