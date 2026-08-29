//! The floating-point numeric layer (Glulx: Floating-Point
//! Numbers, Double-Precision Floating-Point Numbers).
//!
//! Floats are IEEE-754 singles held in one 32-bit value; doubles
//! are IEEE-754 doubles split across two. All arithmetic is
//! computed in double precision and singles round on the way back
//! out -- the reference's architecture, kept so the two
//! implementations agree to the bit. Rust's native f64 already
//! speaks IEEE where CPython raises -- division by zero, sqrt of
//! a negative, log of zero -- so most of the reference's
//! exception-wrapping dissolves; what remains here are the
//! semantics that needed deciding: the saturating conversions
//! with glulxe's boundary quirk, round-half-even for the nearest
//! conversion (the reference rounds Python's way), pow's
//! guaranteed special cases, and the jfeq closeness rules.
//!
//! Word order: a double argument is L1:L2, high word first. A
//! double *result* stores as S2:S1 -- the low word goes to the
//! first store operand. The spec is explicit about the asymmetry
//! (Glulx: Double-Precision Math).

/// glulxe compares against 2147483647.0 in both directions rather
/// than -2147483648.0; matched so the boundary behaves
/// identically.
const SATURATION: f64 = 2_147_483_647.0;

const INT_MAX: u32 = 0x7FFF_FFFF;
const INT_MIN: u32 = 0x8000_0000;

/// Pack a value into an IEEE-754 single. Values too large for
/// single precision become infinity rather than an error, which
/// is what the arithmetic opcodes promise on overflow.
pub fn encode_float(value: f64) -> u32 {
    (value as f32).to_bits()
}

/// Read a 32-bit value as an IEEE-754 single, widened for the
/// double-precision arithmetic every operation runs in.
pub fn decode_float(bits: u32) -> f64 {
    f64::from(f32::from_bits(bits))
}

/// Pack a value into an IEEE-754 double, as (high, low).
pub fn encode_double(value: f64) -> (u32, u32) {
    let bits = value.to_bits();

    ((bits >> 32) as u32, bits as u32)
}

/// Read a high and low word pair as an IEEE-754 double.
pub fn decode_double(high: u32, low: u32) -> f64 {
    f64::from_bits((u64::from(high) << 32) | u64::from(low))
}

/// The IEEE sign bit, set for -0.0 and for a negative NaN.
pub fn negative(value: f64) -> bool {
    value.is_sign_negative()
}

/// A float as a 32-bit integer, saturated (Glulx: Floating-Point
/// Math); `nearest` rounds ties to even, as the reference does.
pub fn to_int(value: f64, nearest: bool) -> u32 {
    if value.is_sign_negative() {
        if value.is_nan() || value.is_infinite() || value < -SATURATION {
            return INT_MIN;
        }
    } else if value.is_nan() || value.is_infinite() || value > SATURATION {
        return INT_MAX;
    }

    let rounded = if nearest {
        value.round_ties_even()
    } else {
        value.trunc()
    };

    rounded as i64 as u32
}

/// Power, with the special cases C's own pow does not guarantee:
/// one to any power, anything to the zeroth, and minus one to an
/// infinite power (the three glulxe adds by hand), plus the
/// zero-base negative-exponent infinities CPython refuses.
pub fn pow(base: f64, exponent: f64) -> f64 {
    if base == 1.0 || exponent == 0.0 {
        return 1.0;
    }

    if base == -1.0 && exponent.is_infinite() {
        return 1.0;
    }

    if base == 0.0 && exponent < 0.0 {
        let odd_integer =
            exponent.is_finite() && exponent == exponent.trunc() && (exponent as i64) % 2 != 0;

        if base.is_sign_negative() && odd_integer {
            return f64::NEG_INFINITY;
        }

        return f64::INFINITY;
    }

    base.powf(exponent)
}

/// The (remainder, quotient) pair fmod and dmod speak: an
/// infinite divisor leaves the value alone with a zero quotient;
/// an infinite dividend or a zero divisor gives NaN for both
/// (Glulx: Floating-Point Math).
pub fn modulo(a: f64, b: f64) -> (f64, f64) {
    if a.is_nan() || b.is_nan() || a.is_infinite() || b == 0.0 {
        return (f64::NAN, f64::NAN);
    }

    if b.is_infinite() {
        return (a, 0.0);
    }

    let remainder = a % b;

    (remainder, (a - remainder) / b)
}

/// The jfeq and jdeq test (Glulx: Floating-Point Comparisons):
/// infinities are settled before epsilon is consulted -- two
/// infinities are equal exactly when their signs match, whatever
/// epsilon says. Only then does an infinite epsilon make
/// everything else equal.
pub fn close(a: f64, b: f64, epsilon: f64) -> bool {
    if a.is_nan() || b.is_nan() || epsilon.is_nan() {
        return false;
    }

    if a.is_infinite() && b.is_infinite() {
        return a.is_sign_negative() == b.is_sign_negative();
    }

    if epsilon.is_infinite() {
        return true;
    }

    (a - b).abs() <= epsilon.abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden vectors from certify/floats_oracle.py.

    #[test]
    fn floats_encode_and_decode_to_the_bit() {
        assert_eq!(encode_float(0.0), 0x0000_0000);
        assert_eq!(encode_float(-0.0), 0x8000_0000);
        assert_eq!(encode_float(1.5), 0x3FC0_0000);
        assert_eq!(encode_float(-2.25), 0xC010_0000);
        assert_eq!(encode_float(3.4e38), 0x7F7F_C99E);
        assert_eq!(encode_float(-3.4e39), 0xFF80_0000);
        assert_eq!(encode_float(1e-45), 0x0000_0001);

        assert_eq!(decode_float(0x3F80_0000), 1.0);
        assert_eq!(decode_float(0xBF80_0000), -1.0);
        assert_eq!(decode_float(0x7F80_0000), f64::INFINITY);
        assert!(decode_float(0x7FC0_0000).is_nan());
        assert_eq!(decode_float(0x0000_0001), 1.401298464324817e-45);
    }

    #[test]
    fn doubles_split_high_then_low() {
        let (high, low) = encode_double(1.5);

        assert_eq!((high, low), (0x3FF8_0000, 0x0000_0000));
        assert_eq!(decode_double(high, low), 1.5);
        assert_eq!(decode_double(0xBFF0_0000, 0), -1.0);
    }

    #[test]
    fn to_int_saturates_and_rounds_ties_to_even() {
        assert_eq!(to_int(0.5, true), 0x0000_0000);
        assert_eq!(to_int(1.5, true), 0x0000_0002);
        assert_eq!(to_int(2.5, true), 0x0000_0002);
        assert_eq!(to_int(-0.5, true), 0x0000_0000);
        assert_eq!(to_int(-1.5, true), 0xFFFF_FFFE);
        assert_eq!(to_int(2.7, false), 0x0000_0002);
        assert_eq!(to_int(-2.7, false), 0xFFFF_FFFE);

        assert_eq!(to_int(2_147_483_646.5, true), 0x7FFF_FFFE);
        assert_eq!(to_int(2_147_483_647.0, true), 0x7FFF_FFFF);
        assert_eq!(to_int(2_147_483_648.0, true), 0x7FFF_FFFF);
        assert_eq!(to_int(-2_147_483_647.5, true), 0x8000_0000);
        assert_eq!(to_int(-2_147_483_648.0, true), 0x8000_0000);
        assert_eq!(to_int(-2_147_483_649.0, true), 0x8000_0000);

        assert_eq!(to_int(f64::INFINITY, true), 0x7FFF_FFFF);
        assert_eq!(to_int(f64::NEG_INFINITY, false), 0x8000_0000);
        assert_eq!(to_int(f64::NAN, true), 0x7FFF_FFFF);
    }

    #[test]
    fn modulo_speaks_its_pairs() {
        assert_eq!(modulo(7.5, 2.0), (1.5, 3.0));
        assert_eq!(modulo(-7.5, 2.0), (-1.5, -3.0));
        assert_eq!(modulo(7.5, -2.0), (1.5, -3.0));
        assert_eq!(modulo(-7.5, -2.0), (-1.5, 3.0));
        assert_eq!(modulo(1.0, f64::INFINITY), (1.0, 0.0));

        let (remainder, quotient) = modulo(f64::INFINITY, 2.0);
        assert!(remainder.is_nan() && quotient.is_nan());

        let (remainder, quotient) = modulo(1.0, 0.0);
        assert!(remainder.is_nan() && quotient.is_nan());
    }

    #[test]
    fn pow_keeps_its_promised_specials() {
        assert_eq!(pow(1.0, f64::NAN), 1.0);
        assert_eq!(pow(f64::NAN, 0.0), 1.0);
        assert_eq!(pow(-1.0, f64::INFINITY), 1.0);
        assert_eq!(pow(0.0, -3.0), f64::INFINITY);
        assert_eq!(pow(-0.0, -3.0), f64::NEG_INFINITY);
        assert_eq!(pow(-0.0, -2.0), f64::INFINITY);
        assert_eq!(pow(-2.0, 3.0), -8.0);
        assert_eq!(pow(1e300, 3.0), f64::INFINITY);
        assert_eq!(pow(-1e300, 3.0), f64::NEG_INFINITY);
        assert!(pow(-2.0, 0.5).is_nan());
    }

    #[test]
    fn close_settles_infinities_before_epsilon() {
        assert!(close(1.0, 1.05, 0.1));
        assert!(!close(1.0, 1.2, 0.1));

        // |1.0 - 1.1| in double is a hair over 0.1: the reference's
        // own answer, pinned as it computes it.
        assert!(!close(1.0, 1.1, -0.1));

        assert!(close(f64::INFINITY, f64::INFINITY, 0.0));
        assert!(!close(f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY));
        assert!(close(1.0, 2.0, f64::INFINITY));
        assert!(!close(f64::NAN, f64::NAN, f64::INFINITY));
    }

    #[test]
    fn rust_native_ieee_matches_the_reference_wrappers() {
        // The cases the reference wraps by hand come free here.
        assert_eq!((-0.5f64).ceil().to_bits(), (-0.0f64).to_bits());
        assert_eq!(0.0f64.sqrt().to_bits(), 0.0f64.to_bits());
        assert_eq!((-0.0f64).sqrt().to_bits(), (-0.0f64).to_bits());
        assert!((-1.0f64).sqrt().is_nan());
        assert_eq!(0.0f64.ln(), f64::NEG_INFINITY);
        assert!((-1.0f64).ln().is_nan());
        assert_eq!(f64::MAX * 2.0, f64::INFINITY);
    }
}
