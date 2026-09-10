/*
   Copyright 2018 Lip Wee Yeo Amano

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

     http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
*/

/**
 * Based on the following, with small tweaks and optimizations:
 *
 * https://github.com/lwYeo/SoliditySHA3Miner/blob/master/SoliditySHA3Miner/
 *   Miner/Kernels/OpenCL/sha3KingKernel.cl
 *
 * Originally modified for openCL processing by lwYeo
 *
 * Original implementor: David Leon Gil
 *
 * License: CC0, attribution kindly requested. Blame taken too, but not
 * liability.
 */

/******** Keccak-f[1600] (for finding efficient Ethereum addresses) ********/

#if defined(METAL_BACKEND)
#include <metal_stdlib>
using namespace metal;
#define THREAD thread
#define CONSTANT constant
#else
#define THREAD
#define CONSTANT __constant
#endif

#define OPENCL_PLATFORM_UNKNOWN 0
#define OPENCL_PLATFORM_AMD 2

#ifndef PLATFORM
#define PLATFORM OPENCL_PLATFORM_UNKNOWN
#endif

#if PLATFORM == OPENCL_PLATFORM_AMD
#pragma OPENCL EXTENSION cl_amd_media_ops : enable
#endif

typedef union _nonce_t {
  ulong uint64_t;
  uint uint32_t[2];
  uchar uint8_t[8];
} nonce_t;

#if PLATFORM == OPENCL_PLATFORM_AMD
static inline ulong rol(const ulong x, const uint s) {
  uint2 output;
  uint2 x2 = as_uint2(x);

  output = (s > 32u) ? amd_bitalign((x2).yx, (x2).xy, 64u - s)
                     : amd_bitalign((x2).xy, (x2).yx, 32u - s);
  return as_ulong(output);
}
#else
static inline ulong rol(const ulong x, const uint s) {
  nonce_t lanes;
  lanes.uint64_t = x;
  const uint lo = lanes.uint32_t[0];
  const uint hi = lanes.uint32_t[1];
  uint out_lo;
  uint out_hi;
  if (s < 32u) {
    out_lo = (lo << s) | (hi >> (32u - s));
    out_hi = (hi << s) | (lo >> (32u - s));
  } else if (s == 32u) {
    out_lo = hi;
    out_hi = lo;
  } else {
    out_lo = (hi << (s - 32u)) | (lo >> (64u - s));
    out_hi = (lo << (s - 32u)) | (hi >> (64u - s));
  }
  lanes.uint32_t[0] = out_lo;
  lanes.uint32_t[1] = out_hi;
  return lanes.uint64_t;
}
#endif

#define rol1(x) rol(x, 1u)

#define theta_(m, n, o)                                                        \
  t = b[m] ^ rol1(b[n]);                                                       \
  a[o + 0] ^= t;                                                               \
  a[o + 5] ^= t;                                                               \
  a[o + 10] ^= t;                                                              \
  a[o + 15] ^= t;                                                              \
  a[o + 20] ^= t;

#define theta()                                                                \
  b[0] = a[0] ^ a[5] ^ a[10] ^ a[15] ^ a[20];                                  \
  b[1] = a[1] ^ a[6] ^ a[11] ^ a[16] ^ a[21];                                  \
  b[2] = a[2] ^ a[7] ^ a[12] ^ a[17] ^ a[22];                                  \
  b[3] = a[3] ^ a[8] ^ a[13] ^ a[18] ^ a[23];                                  \
  b[4] = a[4] ^ a[9] ^ a[14] ^ a[19] ^ a[24];                                  \
  theta_(4, 1, 0);                                                             \
  theta_(0, 2, 1);                                                             \
  theta_(1, 3, 2);                                                             \
  theta_(2, 4, 3);                                                             \
  theta_(3, 0, 4);

#define rhoPi_(m, n)                                                           \
  t = b[0];                                                                    \
  b[0] = a[m];                                                                 \
  a[m] = rol(t, n);

#define rhoPi()                                                                \
  t = a[1];                                                                    \
  b[0] = a[10];                                                                \
  a[10] = rol1(t);                                                             \
  rhoPi_(7, 3);                                                                \
  rhoPi_(11, 6);                                                               \
  rhoPi_(17, 10);                                                              \
  rhoPi_(18, 15);                                                              \
  rhoPi_(3, 21);                                                               \
  rhoPi_(5, 28);                                                               \
  rhoPi_(16, 36);                                                              \
  rhoPi_(8, 45);                                                               \
  rhoPi_(21, 55);                                                              \
  rhoPi_(24, 2);                                                               \
  rhoPi_(4, 14);                                                               \
  rhoPi_(15, 27);                                                              \
  rhoPi_(23, 41);                                                              \
  rhoPi_(19, 56);                                                              \
  rhoPi_(13, 8);                                                               \
  rhoPi_(12, 25);                                                              \
  rhoPi_(2, 43);                                                               \
  rhoPi_(20, 62);                                                              \
  rhoPi_(14, 18);                                                              \
  rhoPi_(22, 39);                                                              \
  rhoPi_(9, 61);                                                               \
  rhoPi_(6, 20);                                                               \
  rhoPi_(1, 44);

#define chi_(n)                                                                \
  b[0] = a[n + 0];                                                             \
  b[1] = a[n + 1];                                                             \
  b[2] = a[n + 2];                                                             \
  b[3] = a[n + 3];                                                             \
  b[4] = a[n + 4];                                                             \
  a[n + 0] = b[0] ^ ((~b[1]) & b[2]);                                          \
  a[n + 1] = b[1] ^ ((~b[2]) & b[3]);                                          \
  a[n + 2] = b[2] ^ ((~b[3]) & b[4]);                                          \
  a[n + 3] = b[3] ^ ((~b[4]) & b[0]);                                          \
  a[n + 4] = b[4] ^ ((~b[0]) & b[1]);

#define chi()                                                                  \
  chi_(0);                                                                     \
  chi_(5);                                                                     \
  chi_(10);                                                                    \
  chi_(15);                                                                    \
  chi_(20);

#define iota(x) a[0] ^= x;

#define iteration(x)                                                           \
  theta();                                                                     \
  rhoPi();                                                                     \
  chi();                                                                       \
  iota(x);

CONSTANT ulong round_constants[23] = {
  0x0000000000000001UL,
  0x0000000000008082UL,
  0x800000000000808aUL,
  0x8000000080008000UL,
  0x000000000000808bUL,
  0x0000000080000001UL,
  0x8000000080008081UL,
  0x8000000000008009UL,
  0x000000000000008aUL,
  0x0000000000000088UL,
  0x0000000080008009UL,
  0x000000008000000aUL,
  0x000000008000808bUL,
  0x800000000000008bUL,
  0x8000000000008089UL,
  0x8000000000008003UL,
  0x8000000000008002UL,
  0x8000000000000080UL,
  0x000000000000800aUL,
  0x800000008000000aUL,
  0x8000000080008081UL,
  0x8000000000008080UL,
  0x0000000080000001UL,
};

static inline void keccakf(THREAD ulong *a) {
  ulong b[5];
  ulong t;

  for (uint round = 0; round < 23; ++round) {
    iteration(round_constants[round]);
  }

  // iteration 24 (partial)

#define o ((THREAD uint *)(a))
  // Theta (partial)
  b[0] = a[0] ^ a[5] ^ a[10] ^ a[15] ^ a[20];
  b[1] = a[1] ^ a[6] ^ a[11] ^ a[16] ^ a[21];
  b[2] = a[2] ^ a[7] ^ a[12] ^ a[17] ^ a[22];
  b[3] = a[3] ^ a[8] ^ a[13] ^ a[18] ^ a[23];
  b[4] = a[4] ^ a[9] ^ a[14] ^ a[19] ^ a[24];

  a[0] ^= b[4] ^ rol1(b[1]);
  a[6] ^= b[0] ^ rol1(b[2]);
  a[12] ^= b[1] ^ rol1(b[3]);
  a[18] ^= b[2] ^ rol1(b[4]);
  a[24] ^= b[3] ^ rol1(b[0]);

  // Rho Pi (partial)
  o[3] = (o[13] >> 20) | (o[12] << 12);
  a[2] = rol(a[12], 43);
  a[3] = rol(a[18], 21);
  a[4] = rol(a[24], 14);

  // Chi (partial)
  o[3] ^= ((~o[5]) & o[7]);
  o[4] ^= ((~o[6]) & o[8]);
  o[5] ^= ((~o[7]) & o[9]);
  o[6] ^= ((~o[8]) & o[0]);
  o[7] ^= ((~o[9]) & o[1]);
#undef o
}

static inline bool hasZeroBytes(THREAD uchar const *d,
                                uint const min_zero_bytes) {
  THREAD uint const *words = (THREAD uint const *)d;
  uint zero_bytes = 0;
  // Per-byte sums cannot carry into the next byte. A high bit stays zero
  // exactly when that original byte is zero; count those five word masks.
#pragma unroll
  for (uint i = 0; i < 5; ++i) {
    const uint word = words[i];
    const uint nonzero = ((word & 0x7f7f7f7fu) + 0x7f7f7f7fu) | word;
    zero_bytes += popcount(~(nonzero | 0x7f7f7f7fu));
  }
  return zero_bytes >= min_zero_bytes;
}

#if defined(METAL_BACKEND)
kernel void hashMessage(constant uint &salt_tail [[buffer(0)]],
                        constant uint &nonce_hi [[buffer(1)]],
                        constant uint &min_zeros [[buffer(2)]],
                        device atomic_uint *solutions [[buffer(3)]],
                        uint global_id [[thread_position_in_grid]]) {
#else
__kernel void hashMessage(uint const salt_tail, uint const nonce_hi,
                          uint const min_zeros,
                          __global volatile ulong *restrict solutions) {
#endif

  ulong spongeBuffer[25];
#define sponge ((THREAD uchar *)spongeBuffer)
#define digest (sponge + 12)
  nonce_t nonce;
#if defined(METAL_BACKEND)
  nonce.uint32_t[0] = global_id;
#else
  nonce.uint32_t[0] = get_global_id(0);
#endif
  nonce.uint32_t[1] = nonce_hi;
  spongeBuffer[0] =
      ((ulong)0xffu << 0)
      | ((ulong)S_1 << 8)
      | ((ulong)S_2 << 16)
      | ((ulong)S_3 << 24)
      | ((ulong)S_4 << 32)
      | ((ulong)S_5 << 40)
      | ((ulong)S_6 << 48)
      | ((ulong)S_7 << 56);
  spongeBuffer[1] =
      ((ulong)S_8 << 0)
      | ((ulong)S_9 << 8)
      | ((ulong)S_10 << 16)
      | ((ulong)S_11 << 24)
      | ((ulong)S_12 << 32)
      | ((ulong)S_13 << 40)
      | ((ulong)S_14 << 48)
      | ((ulong)S_15 << 56);
  spongeBuffer[2] =
      ((ulong)S_16 << 0)
      | ((ulong)S_17 << 8)
      | ((ulong)S_18 << 16)
      | ((ulong)S_19 << 24)
      | ((ulong)S_20 << 32)
      | ((ulong)S_21 << 40)
      | ((ulong)S_22 << 48)
      | ((ulong)S_23 << 56);
  spongeBuffer[3] =
      ((ulong)S_24 << 0)
      | ((ulong)S_25 << 8)
      | ((ulong)S_26 << 16)
      | ((ulong)S_27 << 24)
      | ((ulong)S_28 << 32)
      | ((ulong)S_29 << 40)
      | ((ulong)S_30 << 48)
      | ((ulong)S_31 << 56);
  spongeBuffer[4] =
      ((ulong)S_32 << 0)
      | ((ulong)S_33 << 8)
      | ((ulong)S_34 << 16)
      | ((ulong)S_35 << 24)
      | ((ulong)S_36 << 32)
      | ((ulong)S_37 << 40)
      | ((ulong)S_38 << 48)
      | ((ulong)S_39 << 56);
  spongeBuffer[5] = (ulong)S_40 | ((ulong)salt_tail << 8) | ((ulong)(nonce.uint32_t[0] & 0xffffffu) << 40);
  spongeBuffer[6] =
      (ulong)(nonce.uint32_t[0] >> 24)
      | ((ulong)nonce_hi << 8)
      | ((ulong)S_53 << 40)
      | ((ulong)S_54 << 48)
      | ((ulong)S_55 << 56);
  spongeBuffer[7] =
      ((ulong)S_56 << 0)
      | ((ulong)S_57 << 8)
      | ((ulong)S_58 << 16)
      | ((ulong)S_59 << 24)
      | ((ulong)S_60 << 32)
      | ((ulong)S_61 << 40)
      | ((ulong)S_62 << 48)
      | ((ulong)S_63 << 56);
  spongeBuffer[8] =
      ((ulong)S_64 << 0)
      | ((ulong)S_65 << 8)
      | ((ulong)S_66 << 16)
      | ((ulong)S_67 << 24)
      | ((ulong)S_68 << 32)
      | ((ulong)S_69 << 40)
      | ((ulong)S_70 << 48)
      | ((ulong)S_71 << 56);
  spongeBuffer[9] =
      ((ulong)S_72 << 0)
      | ((ulong)S_73 << 8)
      | ((ulong)S_74 << 16)
      | ((ulong)S_75 << 24)
      | ((ulong)S_76 << 32)
      | ((ulong)S_77 << 40)
      | ((ulong)S_78 << 48)
      | ((ulong)S_79 << 56);
  spongeBuffer[10] =
      ((ulong)S_80 << 0)
      | ((ulong)S_81 << 8)
      | ((ulong)S_82 << 16)
      | ((ulong)S_83 << 24)
      | ((ulong)S_84 << 32)
      | ((ulong)1u << 40);
  spongeBuffer[11] = 0;
  spongeBuffer[12] = 0;
  spongeBuffer[13] = 0;
  spongeBuffer[14] = 0;
  spongeBuffer[15] = 0;
  spongeBuffer[16] = ((ulong)0x80u << 56);
  spongeBuffer[17] = 0;
  spongeBuffer[18] = 0;
  spongeBuffer[19] = 0;
  spongeBuffer[20] = 0;
  spongeBuffer[21] = 0;
  spongeBuffer[22] = 0;
  spongeBuffer[23] = 0;
  spongeBuffer[24] = 0;

  // Apply keccakf
  keccakf(spongeBuffer);

  // determine if the address meets the constraints
  if (hasZeroBytes(digest, min_zeros)) {
    // To be honest, if we are using OpenCL,
    // we just need to write one solution for all practical purposes,
    // since the chance of multiple solutions appearing
    // in a single workset is extremely low.
#if defined(METAL_BACKEND)
    if (atomic_exchange_explicit(&solutions[0], 1u, memory_order_relaxed) ==
        0u) {
      atomic_store_explicit(&solutions[1], nonce.uint32_t[0],
                            memory_order_relaxed);
      atomic_store_explicit(&solutions[2], nonce.uint32_t[1],
                            memory_order_relaxed);
    }
#else
    solutions[0] = nonce.uint64_t;
#endif
  }
}
