/*
 * Microbenchmark: does vectorising ACROSS the batch pay off?
 *
 * In mkp224o a batch of BATCHNUM points is processed identically and
 * independently, so the natural unit of vectorisation is "N independent field
 * elements, one per lane". No cross-lane operations are needed at all: the SIMD
 * version is literally the scalar algorithm with every scalar replaced by a
 * vector.
 *
 * Compared here:
 *   scalar  - multiplication in the field 2^255-19, radix-2^25.5 (10 limbs)
 *   neon    - the same algorithm over 4 independent elements in uint32x4_t lanes
 *
 * Build: cc -O3 -o fe_mul_bench fe_mul_bench.c
 */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <time.h>

#ifdef __ARM_NEON
#include <arm_neon.h>
#define HAVE_NEON 1
#else
#define HAVE_NEON 0
#endif

typedef uint32_t fe[10];

static const uint32_t mask25 = (1u << 25) - 1;
static const uint32_t mask26 = (1u << 26) - 1;

/* ---------------------------------------------------------------- scalar -- */
static void fe_mul_scalar(fe out, const fe a, const fe b)
{
	uint32_t r[10], s[10];
	uint64_t m[10], c;

	for (int i = 0; i < 10; i++) { r[i] = b[i]; s[i] = a[i]; }

	/* odd positions: no doubling */
	m[1] = (uint64_t)r[0]*s[1] + (uint64_t)r[1]*s[0];
	m[3] = (uint64_t)r[0]*s[3] + (uint64_t)r[1]*s[2] + (uint64_t)r[2]*s[1] + (uint64_t)r[3]*s[0];
	m[5] = (uint64_t)r[0]*s[5] + (uint64_t)r[1]*s[4] + (uint64_t)r[2]*s[3] + (uint64_t)r[3]*s[2]
	     + (uint64_t)r[4]*s[1] + (uint64_t)r[5]*s[0];
	m[7] = (uint64_t)r[0]*s[7] + (uint64_t)r[1]*s[6] + (uint64_t)r[2]*s[5] + (uint64_t)r[3]*s[4]
	     + (uint64_t)r[4]*s[3] + (uint64_t)r[5]*s[2] + (uint64_t)r[6]*s[1] + (uint64_t)r[7]*s[0];
	m[9] = (uint64_t)r[0]*s[9] + (uint64_t)r[1]*s[8] + (uint64_t)r[2]*s[7] + (uint64_t)r[3]*s[6]
	     + (uint64_t)r[4]*s[5] + (uint64_t)r[5]*s[4] + (uint64_t)r[6]*s[3] + (uint64_t)r[7]*s[2]
	     + (uint64_t)r[8]*s[1] + (uint64_t)r[9]*s[0];

	r[1] *= 2; r[3] *= 2; r[5] *= 2; r[7] *= 2;

	m[0] = (uint64_t)r[0]*s[0];
	m[2] = (uint64_t)r[0]*s[2] + (uint64_t)r[1]*s[1] + (uint64_t)r[2]*s[0];
	m[4] = (uint64_t)r[0]*s[4] + (uint64_t)r[1]*s[3] + (uint64_t)r[2]*s[2] + (uint64_t)r[3]*s[1]
	     + (uint64_t)r[4]*s[0];
	m[6] = (uint64_t)r[0]*s[6] + (uint64_t)r[1]*s[5] + (uint64_t)r[2]*s[4] + (uint64_t)r[3]*s[3]
	     + (uint64_t)r[4]*s[2] + (uint64_t)r[5]*s[1] + (uint64_t)r[6]*s[0];
	m[8] = (uint64_t)r[0]*s[8] + (uint64_t)r[1]*s[7] + (uint64_t)r[2]*s[6] + (uint64_t)r[3]*s[5]
	     + (uint64_t)r[4]*s[4] + (uint64_t)r[5]*s[3] + (uint64_t)r[6]*s[2] + (uint64_t)r[7]*s[1]
	     + (uint64_t)r[8]*s[0];

	r[1] *= 19; r[2] *= 19; r[3] = (r[3] / 2) * 19; r[4] *= 19;
	r[5] = (r[5] / 2) * 19; r[6] *= 19; r[7] = (r[7] / 2) * 19; r[8] *= 19; r[9] *= 19;

	m[1] += (uint64_t)r[9]*s[2] + (uint64_t)r[8]*s[3] + (uint64_t)r[7]*s[4] + (uint64_t)r[6]*s[5]
	      + (uint64_t)r[5]*s[6] + (uint64_t)r[4]*s[7] + (uint64_t)r[3]*s[8] + (uint64_t)r[2]*s[9];
	m[3] += (uint64_t)r[9]*s[4] + (uint64_t)r[8]*s[5] + (uint64_t)r[7]*s[6] + (uint64_t)r[6]*s[7]
	      + (uint64_t)r[5]*s[8] + (uint64_t)r[4]*s[9];
	m[5] += (uint64_t)r[9]*s[6] + (uint64_t)r[8]*s[7] + (uint64_t)r[7]*s[8] + (uint64_t)r[6]*s[9];
	m[7] += (uint64_t)r[9]*s[8] + (uint64_t)r[8]*s[9];

	r[3] *= 2; r[5] *= 2; r[7] *= 2; r[9] *= 2;

	m[0] += (uint64_t)r[9]*s[1] + (uint64_t)r[8]*s[2] + (uint64_t)r[7]*s[3] + (uint64_t)r[6]*s[4]
	      + (uint64_t)r[5]*s[5] + (uint64_t)r[4]*s[6] + (uint64_t)r[3]*s[7] + (uint64_t)r[2]*s[8]
	      + (uint64_t)r[1]*s[9];
	m[2] += (uint64_t)r[9]*s[3] + (uint64_t)r[8]*s[4] + (uint64_t)r[7]*s[5] + (uint64_t)r[6]*s[6]
	      + (uint64_t)r[5]*s[7] + (uint64_t)r[4]*s[8] + (uint64_t)r[3]*s[9];
	m[4] += (uint64_t)r[9]*s[5] + (uint64_t)r[8]*s[6] + (uint64_t)r[7]*s[7] + (uint64_t)r[6]*s[8]
	      + (uint64_t)r[5]*s[9];
	m[6] += (uint64_t)r[9]*s[7] + (uint64_t)r[8]*s[8] + (uint64_t)r[7]*s[9];
	m[8] += (uint64_t)r[9]*s[9];

	/* carry */
	c = m[0] >> 26; m[0] &= mask26; m[1] += c;
	c = m[1] >> 25; m[1] &= mask25; m[2] += c;
	c = m[2] >> 26; m[2] &= mask26; m[3] += c;
	c = m[3] >> 25; m[3] &= mask25; m[4] += c;
	c = m[4] >> 26; m[4] &= mask26; m[5] += c;
	c = m[5] >> 25; m[5] &= mask25; m[6] += c;
	c = m[6] >> 26; m[6] &= mask26; m[7] += c;
	c = m[7] >> 25; m[7] &= mask25; m[8] += c;
	c = m[8] >> 26; m[8] &= mask26; m[9] += c;
	c = m[9] >> 25; m[9] &= mask25; m[0] += c * 19;
	c = m[0] >> 26; m[0] &= mask26; m[1] += c;

	for (int i = 0; i < 10; i++) out[i] = (uint32_t)m[i];
}

/* ------------------------------------------------------------------ NEON -- */
#if HAVE_NEON
/* Accumulator: 4 lanes of 64-bit values = two uint64x2_t pairs. */
typedef struct { uint64x2_t lo, hi; } acc4;

static inline acc4 acc_zero(void)
{
	acc4 r; r.lo = vdupq_n_u64(0); r.hi = vdupq_n_u64(0); return r;
}
/* a*b across 4 lanes, added into the accumulator */
static inline acc4 acc_mla(acc4 acc, uint32x4_t a, uint32x4_t b)
{
	acc.lo = vmlal_u32(acc.lo, vget_low_u32(a),  vget_low_u32(b));
	acc.hi = vmlal_u32(acc.hi, vget_high_u32(a), vget_high_u32(b));
	return acc;
}
static inline acc4 acc_add(acc4 x, acc4 y)
{
	x.lo = vaddq_u64(x.lo, y.lo); x.hi = vaddq_u64(x.hi, y.hi); return x;
}
/* out/a/b: SoA, 10 limbs, each a uint32x4_t (4 independent field elements) */
static void fe_mul_neon4(uint32x4_t *out, const uint32x4_t *a, const uint32x4_t *b)
{
	uint32x4_t r[10], s[10];
	acc4 m[10];
	const uint32x4_t v19 = vdupq_n_u32(19);

	for (int i = 0; i < 10; i++) { r[i] = b[i]; s[i] = a[i]; }
	for (int i = 0; i < 10; i++) m[i] = acc_zero();

	m[1] = acc_mla(acc_mla(m[1], r[0], s[1]), r[1], s[0]);
	m[3] = acc_mla(acc_mla(acc_mla(acc_mla(m[3], r[0],s[3]), r[1],s[2]), r[2],s[1]), r[3],s[0]);
	m[5] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[5], r[0],s[5]), r[1],s[4]), r[2],s[3]), r[3],s[2]), r[4],s[1]), r[5],s[0]);
	m[7] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[7], r[0],s[7]), r[1],s[6]), r[2],s[5]), r[3],s[4]), r[4],s[3]), r[5],s[2]), r[6],s[1]), r[7],s[0]);
	m[9] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[9], r[0],s[9]), r[1],s[8]), r[2],s[7]), r[3],s[6]), r[4],s[5]), r[5],s[4]), r[6],s[3]), r[7],s[2]), r[8],s[1]), r[9],s[0]);

	r[1] = vshlq_n_u32(r[1],1); r[3] = vshlq_n_u32(r[3],1);
	r[5] = vshlq_n_u32(r[5],1); r[7] = vshlq_n_u32(r[7],1);

	m[0] = acc_mla(m[0], r[0], s[0]);
	m[2] = acc_mla(acc_mla(acc_mla(m[2], r[0],s[2]), r[1],s[1]), r[2],s[0]);
	m[4] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[4], r[0],s[4]), r[1],s[3]), r[2],s[2]), r[3],s[1]), r[4],s[0]);
	m[6] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[6], r[0],s[6]), r[1],s[5]), r[2],s[4]), r[3],s[3]), r[4],s[2]), r[5],s[1]), r[6],s[0]);
	m[8] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[8], r[0],s[8]), r[1],s[7]), r[2],s[6]), r[3],s[5]), r[4],s[4]), r[5],s[3]), r[6],s[2]), r[7],s[1]), r[8],s[0]);

	r[1] = vmulq_u32(r[1], v19);
	r[2] = vmulq_u32(r[2], v19);
	r[3] = vmulq_u32(vshrq_n_u32(r[3],1), v19);
	r[4] = vmulq_u32(r[4], v19);
	r[5] = vmulq_u32(vshrq_n_u32(r[5],1), v19);
	r[6] = vmulq_u32(r[6], v19);
	r[7] = vmulq_u32(vshrq_n_u32(r[7],1), v19);
	r[8] = vmulq_u32(r[8], v19);
	r[9] = vmulq_u32(r[9], v19);

	m[1] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[1], r[9],s[2]), r[8],s[3]), r[7],s[4]), r[6],s[5]), r[5],s[6]), r[4],s[7]), r[3],s[8]), r[2],s[9]);
	m[3] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[3], r[9],s[4]), r[8],s[5]), r[7],s[6]), r[6],s[7]), r[5],s[8]), r[4],s[9]);
	m[5] = acc_mla(acc_mla(acc_mla(acc_mla(m[5], r[9],s[6]), r[8],s[7]), r[7],s[8]), r[6],s[9]);
	m[7] = acc_mla(acc_mla(m[7], r[9],s[8]), r[8],s[9]);

	r[3] = vshlq_n_u32(r[3],1); r[5] = vshlq_n_u32(r[5],1);
	r[7] = vshlq_n_u32(r[7],1); r[9] = vshlq_n_u32(r[9],1);

	m[0] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[0], r[9],s[1]), r[8],s[2]), r[7],s[3]), r[6],s[4]), r[5],s[5]), r[4],s[6]), r[3],s[7]), r[2],s[8]), r[1],s[9]);
	m[2] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[2], r[9],s[3]), r[8],s[4]), r[7],s[5]), r[6],s[6]), r[5],s[7]), r[4],s[8]), r[3],s[9]);
	m[4] = acc_mla(acc_mla(acc_mla(acc_mla(acc_mla(m[4], r[9],s[5]), r[8],s[6]), r[7],s[7]), r[6],s[8]), r[5],s[9]);
	m[6] = acc_mla(acc_mla(acc_mla(m[6], r[9],s[7]), r[8],s[8]), r[7],s[9]);
	m[8] = acc_mla(m[8], r[9], s[9]);

	/* carry: per lane, the same shifts as in the scalar version */
	uint64x2_t c_lo, c_hi;
	const uint64x2_t k26 = vdupq_n_u64(mask26), k25 = vdupq_n_u64(mask25);
	#define CARRY(i, j, sh, msk) \
		c_lo = vshrq_n_u64(m[i].lo, sh); c_hi = vshrq_n_u64(m[i].hi, sh); \
		m[i].lo = vandq_u64(m[i].lo, msk); m[i].hi = vandq_u64(m[i].hi, msk); \
		m[j].lo = vaddq_u64(m[j].lo, c_lo); m[j].hi = vaddq_u64(m[j].hi, c_hi);

	CARRY(0,1,26,k26) CARRY(1,2,25,k25) CARRY(2,3,26,k26) CARRY(3,4,25,k25)
	CARRY(4,5,26,k26) CARRY(5,6,25,k25) CARRY(6,7,26,k26) CARRY(7,8,25,k25)
	CARRY(8,9,26,k26)
	c_lo = vshrq_n_u64(m[9].lo, 25); c_hi = vshrq_n_u64(m[9].hi, 25);
	m[9].lo = vandq_u64(m[9].lo, k25); m[9].hi = vandq_u64(m[9].hi, k25);
	/* NEON cannot do 64x64; x*19 = (x<<4) + (x<<1) + x */
	#define MUL19(v) vaddq_u64(vaddq_u64(vshlq_n_u64((v),4), vshlq_n_u64((v),1)), (v))
	m[0].lo = vaddq_u64(m[0].lo, MUL19(c_lo));
	m[0].hi = vaddq_u64(m[0].hi, MUL19(c_hi));
	#undef MUL19
	CARRY(0,1,26,k26)
	#undef CARRY

	for (int i = 0; i < 10; i++) {
		uint32x2_t lo = vmovn_u64(m[i].lo);
		uint32x2_t hi = vmovn_u64(m[i].hi);
		out[i] = vcombine_u32(lo, hi);
	}
}
#endif

/* --------------------------------------------- scalar, radix 2^51 (64-bit) --
 * This is the representation ed25519-donna actually uses on arm64: 5 limbs of
 * 51 bits, ~25 multiplications instead of ~100. The honest baseline for NEON.
 */
typedef uint64_t fe51[5];
static const uint64_t mask51 = ((uint64_t)1 << 51) - 1;

static void fe51_mul(fe51 out, const fe51 a, const fe51 b)
{
	typedef unsigned __int128 u128;
	u128 t0,t1,t2,t3,t4;
	uint64_t r0=b[0],r1=b[1],r2=b[2],r3=b[3],r4=b[4];
	uint64_t s0=a[0],s1=a[1],s2=a[2],s3=a[3],s4=a[4];
	uint64_t c;

	t0 = (u128)r0*s0 + (u128)(19*r4)*s1 + (u128)(19*r3)*s2 + (u128)(19*r2)*s3 + (u128)(19*r1)*s4;
	t1 = (u128)r1*s0 + (u128)r0*s1      + (u128)(19*r4)*s2 + (u128)(19*r3)*s3 + (u128)(19*r2)*s4;
	t2 = (u128)r2*s0 + (u128)r1*s1      + (u128)r0*s2      + (u128)(19*r4)*s3 + (u128)(19*r3)*s4;
	t3 = (u128)r3*s0 + (u128)r2*s1      + (u128)r1*s2      + (u128)r0*s3      + (u128)(19*r4)*s4;
	t4 = (u128)r4*s0 + (u128)r3*s1      + (u128)r2*s2      + (u128)r1*s3      + (u128)r0*s4;

	c = (uint64_t)(t0 >> 51); out[0] = (uint64_t)t0 & mask51; t1 += c;
	c = (uint64_t)(t1 >> 51); out[1] = (uint64_t)t1 & mask51; t2 += c;
	c = (uint64_t)(t2 >> 51); out[2] = (uint64_t)t2 & mask51; t3 += c;
	c = (uint64_t)(t3 >> 51); out[3] = (uint64_t)t3 & mask51; t4 += c;
	c = (uint64_t)(t4 >> 51); out[4] = (uint64_t)t4 & mask51;
	out[0] += c * 19;
	c = out[0] >> 51; out[0] &= mask51; out[1] += c;
}

/* ---------------------------------------------------------------- driver -- */
static uint64_t now_ns(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

static uint32_t rng_state = 12345;
static uint32_t rnd(void) { rng_state = rng_state * 1664525u + 1013904223u; return rng_state; }

static void fe_random(fe x)
{
	for (int i = 0; i < 10; i++)
		x[i] = rnd() & ((i & 1) ? mask25 : mask26);
}

int main(int argc, char **argv)
{
	const int verify = (argc > 1 && strcmp(argv[1], "verify") == 0);

#if !HAVE_NEON
	printf("NEON is not available on this platform\n");
	return 1;
#else
	fe fa[4], fb[4], fs[4];
	uint32x4_t va[10], vb[10], vo[10];

	for (int k = 0; k < 4; k++) { fe_random(fa[k]); fe_random(fb[k]); }

	/* SoA: limb i of all four elements in one vector */
	for (int i = 0; i < 10; i++) {
		uint32_t ta[4], tb[4];
		for (int k = 0; k < 4; k++) { ta[k] = fa[k][i]; tb[k] = fb[k][i]; }
		va[i] = vld1q_u32(ta); vb[i] = vld1q_u32(tb);
	}

	for (int k = 0; k < 4; k++) fe_mul_scalar(fs[k], fa[k], fb[k]);
	fe_mul_neon4(vo, va, vb);

	int bad = 0;
	for (int i = 0; i < 10; i++) {
		uint32_t t[4]; vst1q_u32(t, vo[i]);
		for (int k = 0; k < 4; k++)
			if (t[k] != fs[k][i]) { bad++;
				if (verify) printf("mismatch: limb %d lane %d: neon=%u scalar=%u\n",
				                   i, k, t[k], fs[k][i]); }
	}
	printf("correctness (neon == scalar across all 4 lanes): %s\n", bad ? "FAIL" : "OK");

	if (verify) {
		/* limbs for external cross-checking against arithmetic mod 2^255-19 */
		for (int k = 0; k < 4; k++) {
			printf("A%d", k); for (int i=0;i<10;i++) printf(" %u", fa[k][i]); printf("\n");
			printf("B%d", k); for (int i=0;i<10;i++) printf(" %u", fb[k][i]); printf("\n");
			printf("R%d", k); for (int i=0;i<10;i++) printf(" %u", fs[k][i]); printf("\n");
		}
		return bad ? 1 : 0;
	}
	if (bad) return 1;

	/* --- baseline: scalar radix 2^51, as donna uses on arm64 --- */
	fe51 fa51[4], fb51[4], fo51[4];
	for (int k = 0; k < 4; k++)
		for (int i = 0; i < 5; i++) {
			fa51[k][i] = ((uint64_t)rnd() << 20 | rnd()) & mask51;
			fb51[k][i] = ((uint64_t)rnd() << 20 | rnd()) & mask51;
		}

	if (verify) { /* unreachable */ }

	const long iters = 20000000;

	uint64_t t4a = now_ns();
	for (long n = 0; n < iters; n++)
		for (int k = 0; k < 4; k++) fe51_mul(fo51[k], fa51[k], fb51[k]);
	uint64_t t4b = now_ns();
	double fe51_ns = (double)(t4b - t4a) / (double)(iters * 4);

	/* scalar: 4 independent multiplications per iteration - the same work one
	   iteration of the NEON version does */
	uint64_t t0 = now_ns();
	for (long n = 0; n < iters; n++)
		for (int k = 0; k < 4; k++) fe_mul_scalar(fs[k], fa[k], fb[k]);
	uint64_t t1 = now_ns();

	uint64_t t2 = now_ns();
	for (long n = 0; n < iters; n++) fe_mul_neon4(vo, va, vb);
	uint64_t t3 = now_ns();

	double scalar_ns = (double)(t1 - t0) / (double)(iters * 4);
	double neon_ns   = (double)(t3 - t2) / (double)(iters * 4);

	printf("\nfield multiplications:              %ld\n", iters * 4);
	printf("scalar radix 2^25.5 (32-bit):   %7.2f ns\n", scalar_ns);
	printf("scalar radix 2^51   (64-bit):   %7.2f ns   <- donna baseline on arm64\n", fe51_ns);
	printf("NEON 4 lanes, radix 2^25.5:     %7.2f ns\n", neon_ns);
	printf("\nNEON versus 32-bit scalar:      %7.2fx\n", scalar_ns / neon_ns);
	printf("NEON versus the REAL baseline:  %7.2fx\n", fe51_ns / neon_ns);

	/* keeps the optimiser from discarding the computation */
	uint32_t sink = 0;
	for (int i = 0; i < 10; i++) { uint32_t t[4]; vst1q_u32(t, vo[i]); sink ^= t[0] ^ fs[0][i]; }
	for (int i = 0; i < 5; i++) sink ^= (uint32_t)fo51[0][i];
	if (sink == 0xdeadbeef) printf("(unreachable) %u\n", sink);
	return 0;
#endif
}
