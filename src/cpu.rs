//! Process-wide immutable CPU capabilities; no parsing or caller data is shared.

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
pub(crate) fn avx2() -> bool {
    capabilities() & 2 != 0
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
pub(crate) fn ssse3() -> bool {
    capabilities() & 1 != 0
}

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
fn capabilities() -> u8 {
    #[cfg(miri)]
    {
        // The interpreter validates kernels selected by its configured target
        // features; it never inherits the executor host's CPUID capabilities.
        u8::from(cfg!(target_feature = "ssse3")) | (u8::from(cfg!(target_feature = "avx2")) << 1)
    }
    #[cfg(not(miri))]
    {
        use core::sync::atomic::{AtomicU8, Ordering};
        // Only capability bits are published. There is no associated memory,
        // so Relaxed is sufficient. Racing first calls may repeat detection;
        // every stored value is independently valid for the process CPU.
        static SELECTED: AtomicU8 = AtomicU8::new(0);
        let selected = SELECTED.load(Ordering::Relaxed);
        if selected != 0 {
            return selected;
        }
        let mut selected = 0x80;
        // SAFETY: CPUID is available on all x86_64 CPUs. XGETBV executes only
        // when CPUID reports OSXSAVE and AVX; its result must enable both XMM
        // and YMM state. Leaf 7 is queried only when the maximum leaf allows
        // it. AVX2 selection additionally proves the SSSE3 instructions used
        // by the shared packing kernels.
        unsafe {
            use core::arch::x86_64::{__cpuid, __cpuid_count, _xgetbv};
            let basic = __cpuid(1);
            if basic.ecx & (1 << 9) != 0 {
                selected |= 1;
                if basic.ecx & ((1 << 27) | (1 << 28)) == ((1 << 27) | (1 << 28))
                    && _xgetbv(0) & 6 == 6
                    && __cpuid(0).eax >= 7
                    && __cpuid_count(7, 0).ebx & (1 << 5) != 0
                {
                    selected |= 2;
                }
            }
        }
        SELECTED.store(selected, Ordering::Relaxed);
        selected
    }
}
