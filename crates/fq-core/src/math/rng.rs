//! Pseudo-random numbers for Monte Carlo.
//!
//! A self-contained xoshiro256** (Blackman & Vigna) with splitmix64
//! seeding, so the workspace has no external RNG dependency and paths are
//! reproducible across platforms. Gaussians come from the Marsaglia polar
//! method with the spare cached.

#[derive(Debug, Clone, PartialEq)]
pub struct Xoshiro256StarStar {
    s: [u64; 4],
    spare: Option<f64>,
}

impl Xoshiro256StarStar {
    pub fn seed_from_u64(seed: u64) -> Self {
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Self { s: [next(), next(), next(), next()], spare: None }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in (0, 1), never exactly 0 or 1.
    pub fn next_f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) * (1.0 / (1u64 << 53) as f64)
    }

    /// Standard normal by the Marsaglia polar method (pairs cached).
    pub fn next_gaussian(&mut self) -> f64 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        loop {
            let u = 2.0 * self.next_f64() - 1.0;
            let v = 2.0 * self.next_f64() - 1.0;
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                let m = (-2.0 * s.ln() / s).sqrt();
                self.spare = Some(v * m);
                return u * m;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reproducible_and_in_range() {
        let mut a = Xoshiro256StarStar::seed_from_u64(7);
        let mut b = Xoshiro256StarStar::seed_from_u64(7);
        for _ in 0..1000 {
            let u = a.next_f64();
            assert!(u > 0.0 && u < 1.0);
            assert!((u - b.next_f64()).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn gaussian_moments() {
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        let n = 200_000;
        let (mut m1, mut m2) = (0.0, 0.0);
        for _ in 0..n {
            let z = rng.next_gaussian();
            m1 += z;
            m2 += z * z;
        }
        let mean = m1 / n as f64;
        let var = m2 / n as f64 - mean * mean;
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!((var - 1.0).abs() < 0.01, "var {var}");
    }
}
