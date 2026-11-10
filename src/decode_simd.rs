//! Private, bounded SIMD kernels. The scalar caller owns MIME syntax and tails.

pub(super) struct Prefix {
    pub(super) read: usize,
    pub(super) written: usize,
}

#[derive(Clone, Copy)]
pub(super) enum Backend {
    Unselected,
    Scalar,
    #[cfg(all(feature = "simd", target_arch = "x86_64"))]
    Ssse3,
    #[cfg(all(feature = "simd", target_arch = "x86_64"))]
    Avx2,
    #[cfg(all(feature = "simd", target_arch = "aarch64", target_feature = "neon"))]
    Neon,
}

impl Backend {
    pub(super) fn detect() -> Self {
        #[cfg(all(feature = "simd", target_arch = "x86_64"))]
        {
            if crate::cpu::avx2() {
                return Self::Avx2;
            }
            if crate::cpu::ssse3() {
                return Self::Ssse3;
            }
        }
        #[cfg(all(feature = "simd", target_arch = "aarch64", target_feature = "neon"))]
        {
            return Self::Neon;
        }
        #[allow(unreachable_code)]
        Self::Scalar
    }

    pub(super) fn base64_prefix(self, input: &[u8], output: &mut [u8]) -> Prefix {
        match self {
            #[cfg(all(feature = "simd", target_arch = "x86_64"))]
            Self::Avx2 => {
                // SAFETY: Only detect() constructs this variant, after proving
                // AVX2 and OS support. The kernel checks both slice lengths
                // before every unaligned load/store; slices cannot alias.
                unsafe { x86::avx2(input, output) }
            }
            #[cfg(all(feature = "simd", target_arch = "x86_64"))]
            Self::Ssse3 => {
                // SAFETY: detect() proved SSSE3; the kernel bounds each access.
                unsafe { x86::ssse3(input, output) }
            }
            #[cfg(all(feature = "simd", target_arch = "aarch64", target_feature = "neon"))]
            Self::Neon => {
                // SAFETY: This variant exists only when the target guarantees
                // NEON; the kernel bounds all interleaved loads and stores.
                unsafe { arm::base64(input, output) }
            }
            _ => {
                let _ = (input, output);
                Prefix {
                    read: 0,
                    written: 0,
                }
            }
        }
    }
}

#[inline]
pub(super) fn literal_prefix(input: &[u8]) -> usize {
    #[cfg(all(feature = "simd", target_arch = "x86_64"))]
    {
        // SAFETY: SSE2 is an x86_64 baseline feature. The kernel only reads
        // complete 16-byte blocks within input and uses no aligned access.
        unsafe { x86::literal(input) }
    }
    #[cfg(all(feature = "simd", target_arch = "aarch64", target_feature = "neon"))]
    {
        // SAFETY: The target guarantees NEON and the kernel bounds every load.
        unsafe { arm::literal(input) }
    }
    #[cfg(not(any(
        all(feature = "simd", target_arch = "x86_64"),
        all(feature = "simd", target_arch = "aarch64", target_feature = "neon")
    )))]
    {
        let _ = input;
        0
    }
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
mod x86 {
    use super::Prefix;
    use core::arch::x86_64::*;

    // Each low-nibble entry contains the bit of every valid high-nibble
    // class. Their intersection is nonzero exactly for A-Z, a-z, 0-9, +, /.
    const LOW: [i8; 16] = [10, 14, 14, 14, 14, 14, 14, 14, 14, 14, 12, 5, 4, 4, 4, 5];
    const HIGH: [i8; 16] = [0, 0, 1, 2, 4, 8, 4, 8, 0, 0, 0, 0, 0, 0, 0, 0];
    const ROLL: [i8; 16] = [0, 0, 19, 4, -65, -65, -71, -71, 0, 0, 0, 0, 0, 0, 0, 0];
    const PACK: [i8; 16] = [2, 1, 0, 6, 5, 4, 10, 9, 8, 14, 13, 12, -1, -1, -1, -1];

    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn avx2(input: &[u8], output: &mut [u8]) -> Prefix {
        // SAFETY: The caller proves AVX2 and OS support. Constant arrays have
        // exactly 16 bytes. The loop requires 32 readable and 24 writable
        // bytes before accessing pointers. Stores write precisely 16 + 8
        // bytes, never the unused eight bytes of the packed SIMD register.
        unsafe {
            let low = _mm256_broadcastsi128_si256(_mm_loadu_si128(LOW.as_ptr().cast()));
            let high = _mm256_broadcastsi128_si256(_mm_loadu_si128(HIGH.as_ptr().cast()));
            let roll = _mm256_broadcastsi128_si256(_mm_loadu_si128(ROLL.as_ptr().cast()));
            let pack = _mm256_broadcastsi128_si256(_mm_loadu_si128(PACK.as_ptr().cast()));
            let nibble = _mm256_set1_epi8(15);
            let slash = _mm256_set1_epi8(b'/' as i8);
            let adjust = _mm256_set1_epi8(-3);
            let zero = _mm256_setzero_si256();
            let pairs = _mm256_set1_epi32(0x0140_0140);
            let quads = _mm256_set1_epi32(0x0001_1000);
            let order = _mm256_setr_epi32(0, 1, 2, 4, 5, 6, 3, 7);
            let mut read = 0;
            let mut written = 0;
            while input.len() - read >= 32 && output.len() - written >= 24 {
                let text = _mm256_loadu_si256(input.as_ptr().add(read).cast());
                let hi = _mm256_and_si256(_mm256_srli_epi16(text, 4), nibble);
                let lo = _mm256_and_si256(text, nibble);
                let valid =
                    _mm256_and_si256(_mm256_shuffle_epi8(low, lo), _mm256_shuffle_epi8(high, hi));
                let invalid = _mm256_movemask_epi8(_mm256_cmpeq_epi8(valid, zero)) as u32;
                if invalid != 0 {
                    if matches!(input[read], b' ' | b'\t' | b'\r' | b'\n') {
                        while read < input.len()
                            && matches!(input[read], b' ' | b'\t' | b'\r' | b'\n')
                        {
                            read += 1;
                        }
                        continue;
                    }
                    if invalid.trailing_zeros() < 4 {
                        break;
                    }
                }
                let offset = _mm256_add_epi8(
                    _mm256_shuffle_epi8(roll, hi),
                    _mm256_and_si256(_mm256_cmpeq_epi8(text, slash), adjust),
                );
                let values = _mm256_add_epi8(text, offset);
                let words = _mm256_maddubs_epi16(values, pairs);
                let bytes = _mm256_shuffle_epi8(_mm256_madd_epi16(words, quads), pack);
                let bytes = _mm256_permutevar8x32_epi32(bytes, order);
                let dst = output.as_mut_ptr().add(written);
                if invalid == 0 {
                    _mm_storeu_si128(dst.cast(), _mm256_castsi256_si128(bytes));
                    _mm_storel_epi64(dst.add(16).cast(), _mm256_extracti128_si256(bytes, 1));
                    // Keep the common stride independent of validation bits.
                    read += 32;
                    written += 24;
                    continue;
                }
                // A fold can end a vector partway through. Decode only whole
                // valid quartets before it; split quartets use scalar syntax.
                let groups = invalid.trailing_zeros() as usize / 4;
                if groups == 3 {
                    // A 76-column MIME line leaves twelve symbols after two
                    // full vectors. Its nine output bytes need exactly 8 + 1
                    // bounded stores, avoiding a temporary and variable copy.
                    let lower = _mm256_castsi256_si128(bytes);
                    _mm_storel_epi64(dst.cast(), lower);
                    dst.add(8)
                        .write(_mm_cvtsi128_si32(_mm_srli_si128(lower, 8)) as u8);
                } else {
                    let mut packed = [0u8; 32];
                    _mm256_storeu_si256(packed.as_mut_ptr().cast(), bytes);
                    output[written..written + groups * 3].copy_from_slice(&packed[..groups * 3]);
                }
                read += groups * 4;
                written += groups * 3;
                // Consume a fold now; loading it as another vector would
                // repeat all classification work before taking the same path.
                while read < input.len() && matches!(input[read], b' ' | b'\t' | b'\r' | b'\n') {
                    read += 1;
                }
            }
            Prefix { read, written }
        }
    }

    #[target_feature(enable = "ssse3")]
    pub(super) unsafe fn ssse3(input: &[u8], output: &mut [u8]) -> Prefix {
        // SAFETY: The caller proves SSSE3. All constant loads read 16 bytes;
        // each iteration checks 16 input and 12 output bytes. The stores write
        // precisely eight plus four unaligned bytes inside the output slice.
        unsafe {
            let low = _mm_loadu_si128(LOW.as_ptr().cast());
            let high = _mm_loadu_si128(HIGH.as_ptr().cast());
            let roll = _mm_loadu_si128(ROLL.as_ptr().cast());
            let pack = _mm_loadu_si128(PACK.as_ptr().cast());
            let nibble = _mm_set1_epi8(15);
            let slash = _mm_set1_epi8(b'/' as i8);
            let adjust = _mm_set1_epi8(-3);
            let zero = _mm_setzero_si128();
            let pairs = _mm_set1_epi32(0x0140_0140);
            let quads = _mm_set1_epi32(0x0001_1000);
            let mut read = 0;
            let mut written = 0;
            while input.len() - read >= 16 && output.len() - written >= 12 {
                let text = _mm_loadu_si128(input.as_ptr().add(read).cast());
                let hi = _mm_and_si128(_mm_srli_epi16(text, 4), nibble);
                let lo = _mm_and_si128(text, nibble);
                let valid = _mm_and_si128(_mm_shuffle_epi8(low, lo), _mm_shuffle_epi8(high, hi));
                let invalid = _mm_movemask_epi8(_mm_cmpeq_epi8(valid, zero)) as u16;
                if invalid != 0 {
                    if matches!(input[read], b' ' | b'\t' | b'\r' | b'\n') {
                        while read < input.len()
                            && matches!(input[read], b' ' | b'\t' | b'\r' | b'\n')
                        {
                            read += 1;
                        }
                        continue;
                    }
                    if invalid.trailing_zeros() < 4 {
                        break;
                    }
                }
                let offset = _mm_add_epi8(
                    _mm_shuffle_epi8(roll, hi),
                    _mm_and_si128(_mm_cmpeq_epi8(text, slash), adjust),
                );
                let values = _mm_add_epi8(text, offset);
                let bytes = _mm_shuffle_epi8(
                    _mm_madd_epi16(_mm_maddubs_epi16(values, pairs), quads),
                    pack,
                );
                let dst = output.as_mut_ptr().add(written);
                if invalid == 0 {
                    _mm_storel_epi64(dst.cast(), bytes);
                    dst.add(8)
                        .cast::<i32>()
                        .write_unaligned(_mm_cvtsi128_si32(_mm_srli_si128(bytes, 8)));
                    read += 16;
                    written += 12;
                    continue;
                }
                let groups = invalid.trailing_zeros() as usize / 4;
                if groups == 3 {
                    // Nine output bytes are within the twelve-byte capacity
                    // established by the loop. These stores write exactly 9.
                    _mm_storel_epi64(dst.cast(), bytes);
                    dst.add(8)
                        .write(_mm_cvtsi128_si32(_mm_srli_si128(bytes, 8)) as u8);
                } else {
                    let mut packed = [0u8; 16];
                    _mm_storeu_si128(packed.as_mut_ptr().cast(), bytes);
                    output[written..written + groups * 3].copy_from_slice(&packed[..groups * 3]);
                }
                read += groups * 4;
                written += groups * 3;
                while read < input.len() && matches!(input[read], b' ' | b'\t' | b'\r' | b'\n') {
                    read += 1;
                }
            }
            Prefix { read, written }
        }
    }

    #[target_feature(enable = "sse2")]
    pub(super) unsafe fn literal(input: &[u8]) -> usize {
        // SAFETY: x86_64 guarantees SSE2. Each loop checks the complete 16-byte
        // input block; it performs no writes and no aligned pointer accesses.
        unsafe {
            let mut pos = 0;
            let low = _mm_set1_epi8(31);
            let high = _mm_set1_epi8(127);
            let equals = _mm_set1_epi8(b'=' as i8);
            while input.len() - pos >= 16 {
                let text = _mm_loadu_si128(input.as_ptr().add(pos).cast());
                let printable = _mm_cmpgt_epi8(text, low);
                let special =
                    _mm_or_si128(_mm_cmpeq_epi8(text, equals), _mm_cmpeq_epi8(text, high));
                let mask =
                    (!_mm_movemask_epi8(_mm_andnot_si128(special, printable))) as u32 & 0xffff;
                if mask != 0 {
                    return pos + mask.trailing_zeros() as usize;
                }
                pos += 16;
            }
            pos
        }
    }
}

#[cfg(all(feature = "simd", target_arch = "aarch64", target_feature = "neon"))]
mod arm {
    use super::Prefix;
    use core::arch::aarch64::*;

    #[target_feature(enable = "neon")]
    #[allow(unused_unsafe)] // NEON register intrinsics require unsafe on MSRV 1.74.
    unsafe fn values(text: uint8x16_t) -> (uint8x16_t, uint8x16_t) {
        // SAFETY: Called only by the NEON kernel; all operations are register
        // operations with immediate shifts inside the supported lane width.
        unsafe {
            let upper = vandq_u8(
                vcgeq_u8(text, vdupq_n_u8(b'A')),
                vcleq_u8(text, vdupq_n_u8(b'Z')),
            );
            let lower = vandq_u8(
                vcgeq_u8(text, vdupq_n_u8(b'a')),
                vcleq_u8(text, vdupq_n_u8(b'z')),
            );
            let digit = vandq_u8(
                vcgeq_u8(text, vdupq_n_u8(b'0')),
                vcleq_u8(text, vdupq_n_u8(b'9')),
            );
            let plus = vceqq_u8(text, vdupq_n_u8(b'+'));
            let slash = vceqq_u8(text, vdupq_n_u8(b'/'));
            let valid = vorrq_u8(
                vorrq_u8(upper, lower),
                vorrq_u8(digit, vorrq_u8(plus, slash)),
            );
            let result = vorrq_u8(
                vorrq_u8(
                    vandq_u8(upper, vsubq_u8(text, vdupq_n_u8(65))),
                    vandq_u8(lower, vsubq_u8(text, vdupq_n_u8(71))),
                ),
                vorrq_u8(
                    vandq_u8(digit, vaddq_u8(text, vdupq_n_u8(4))),
                    vorrq_u8(
                        vandq_u8(plus, vdupq_n_u8(62)),
                        vandq_u8(slash, vdupq_n_u8(63)),
                    ),
                ),
            );
            (result, valid)
        }
    }

    #[target_feature(enable = "neon")]
    pub(super) unsafe fn base64(input: &[u8], output: &mut [u8]) -> Prefix {
        // SAFETY: The target guarantees NEON. vld4q reads exactly 64 bytes and
        // vst3q writes exactly 48, both proven available by the loop. Immutable
        // input and mutable output slices are disjoint by safe Rust borrowing.
        unsafe {
            let mut read = 0;
            let mut written = 0;
            loop {
                while read < input.len() && matches!(input[read], b' ' | b'\t' | b'\r' | b'\n') {
                    read += 1;
                }
                if input.len() - read < 64 || output.len() - written < 48 {
                    break;
                }
                let lanes = vld4q_u8(input.as_ptr().add(read));
                let (a, av) = values(lanes.0);
                let (b, bv) = values(lanes.1);
                let (c, cv) = values(lanes.2);
                let (d, dv) = values(lanes.3);
                let valid = vandq_u8(vandq_u8(av, bv), vandq_u8(cv, dv));
                let groups = if vminvq_u8(valid) == 0 {
                    let mut mask = [0u8; 16];
                    vst1q_u8(mask.as_mut_ptr(), valid);
                    mask.iter().position(|b| *b == 0).unwrap_or(16)
                } else {
                    16
                };
                if groups == 0 {
                    break;
                }
                let bytes = uint8x16x3_t(
                    vorrq_u8(vshlq_n_u8(a, 2), vshrq_n_u8(b, 4)),
                    vorrq_u8(vshlq_n_u8(b, 4), vshrq_n_u8(c, 2)),
                    vorrq_u8(vshlq_n_u8(c, 6), d),
                );
                if groups == 16 {
                    vst3q_u8(output.as_mut_ptr().add(written), bytes);
                } else {
                    let mut packed = [0u8; 48];
                    vst3q_u8(packed.as_mut_ptr(), bytes);
                    output[written..written + groups * 3].copy_from_slice(&packed[..groups * 3]);
                }
                read += groups * 4;
                written += groups * 3;
            }
            Prefix { read, written }
        }
    }

    #[target_feature(enable = "neon")]
    pub(super) unsafe fn literal(input: &[u8]) -> usize {
        // SAFETY: Every vector load reads a checked 16-byte input block. This
        // function neither writes output nor forms pointers outside the slice.
        unsafe {
            let mut pos = 0;
            while input.len() - pos >= 16 {
                let text = vld1q_u8(input.as_ptr().add(pos));
                let printable = vandq_u8(
                    vcgeq_u8(text, vdupq_n_u8(32)),
                    vcleq_u8(text, vdupq_n_u8(126)),
                );
                let valid = vbicq_u8(printable, vceqq_u8(text, vdupq_n_u8(b'=')));
                if vminvq_u8(valid) != 255 {
                    break;
                }
                pos += 16;
            }
            pos
        }
    }
}
