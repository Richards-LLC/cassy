# BLAKE3 1.8.6 build override

Cassy vendors the exact crates.io `blake3` 1.8.6 package under
`vendor/blake3-1.8.6` and overrides it with `[patch.crates-io]`. The upstream
package checksum was
`76ae7bad254120e9e4c63bafc385310756f90c484eac0e36b8317cf09cb92a77`.
The upstream licenses and notices are preserved. The only upstream source
change is in `build.rs`: when Cargo enables `CARGO_FEATURE_NO_AVX512`, the build
skips the AVX-512 assembly/intrinsics compilation branch.

Upstream's `no_avx512` feature only makes runtime detection return false. Its
build script still creates and links `libblake3_avx512_assembly.a` whenever the
C compiler accepts AVX-512 flags. That inactive code still violates Cassy's
portable Linux artifact policy, which rejects every EVEX/AVX-512 instruction
in the final executable.

The override does not change the BLAKE3 API or hashing behavior. It retains the
portable, SSE, SSE4.1, and AVX2 implementations and removes only the unreachable
AVX-512 build inputs. AVX-512-capable hosts can therefore see lower throughput
in BLAKE3-heavy indexing and fingerprinting; hashes and stored fingerprints are
unchanged.
